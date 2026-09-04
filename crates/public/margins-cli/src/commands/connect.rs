//! Product-language machine connection orchestration.

use crate::error::CliError;
use margins_workflows::{
    integrations::{
        connect_google_account, connect_granola_account, granola_failure_info,
        list_granola_accounts, redact_secret_text, GoogleAccountStore, GoogleCredentialBackendKind,
        GoogleNativeError, GoogleOAuthMode, GoogleOAuthPresenter, GranolaAccountStore,
        GranolaConnectionMetadata, GranolaCredentialBackendKind, GranolaNativeError,
        GranolaOAuthMode, GranolaOAuthPresenter, IntegrationsStore, REQUIRED_GOOGLE_SCOPES,
    },
    workspace,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

const OFFICIAL_BUILD_REQUIRED: &str = "Google connect requires the official Margins build.";

pub fn granola(
    margins_home: &Path,
    expected_account: Option<&str>,
    headless: bool,
    json_output: bool,
    output: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    #[cfg(debug_assertions)]
    if let Some(error) = synthetic_granola_token_error() {
        return Err(granola_native_error(anyhow::Error::new(error)));
    }
    stderr.flush().map_err(granola_io_error)?;
    let backend = granola_cli_credential_backend();
    let mode = granola_oauth_mode_for_connect(headless);
    let presenter = GranolaCliPresenter { headless };
    let ready = connect_granola_account(margins_home, expected_account, backend, mode, &presenter)
        .map_err(granola_native_error)?;
    if json_output {
        writeln!(
            output,
            "{}",
            json!({
                "schema_version": 1,
                "service": "granola",
                "scope": "machine",
                "authorized": true,
                "account": ready.account,
                "storage": ready.storage.as_str(),
                "access": ready.access,
            })
        )
        .map_err(granola_io_error)?;
    } else {
        writeln!(output, "Granola import authorized: {}", ready.account)
            .map_err(granola_io_error)?;
    }
    Ok(())
}

fn granola_oauth_mode_for_connect(headless: bool) -> GranolaOAuthMode {
    if headless {
        GranolaOAuthMode::HeadlessLoopback {
            redirect_uri: String::new(),
        }
    } else {
        GranolaOAuthMode::BrowserLoopback
    }
}

fn granola_cli_credential_backend() -> GranolaCredentialBackendKind {
    GranolaCredentialBackendKind::File0600
}

#[cfg(debug_assertions)]
fn synthetic_granola_token_error() -> Option<GranolaNativeError> {
    // Debug-only, enumerated fault injection lets the fresh-E2E harness exercise
    // the real CLI error boundary without issuing authorization material.
    let reason = std::env::var("MARGINS_GRANOLA_NATIVE_E2E_TOKEN_ERROR").ok()?;
    let reason = match reason.trim() {
        "access_denied" => "access_denied",
        "expired" => "expired",
        "authorization_pending" => "authorization_pending",
        "slow_down" => "slow_down",
        "invalid_client" => "invalid_client",
        "invalid_grant" => "invalid_grant",
        "invalid_request" => "invalid_request",
        "invalid_scope" => "invalid_scope",
        "unauthorized_client" => "unauthorized_client",
        "unsupported_grant_type" => "unsupported_grant_type",
        "server_rejected" => "server_rejected",
        "transport" => "transport",
        "invalid_response" => "invalid_response",
        "protocol" => "protocol",
        _ => return None,
    };
    Some(GranolaNativeError::OAuthStage {
        stage: "browser_token_exchange",
        reason,
    })
}

struct GranolaCliPresenter {
    headless: bool,
}

impl GranolaOAuthPresenter for GranolaCliPresenter {
    fn present_authorization_url(&self, url: &str, need_callback: bool) -> Result<String, String> {
        if !self.headless {
            open_browser(url).map_err(|error| error.message().to_string())?;
            return Ok(String::new());
        }
        eprintln!("{}", granola_headless_consent_instructions(url));
        if !need_callback {
            return Ok(String::new());
        }
        let callback = prompt_granola_callback().map_err(|error| error.message().to_string())?;
        validate_headless_callback_input(&callback)
    }
}

fn prompt_granola_callback() -> Result<String, CliError> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = rpassword::prompt_password("Final localhost address-bar URL: ")
            .map_err(|error| error.to_string());
        let _ = tx.send(result);
    });
    rx.recv_timeout(granola_headless_prompt_timeout())
        .map_err(|_| {
            CliError::new(
                "granola_oauth_timed_out",
                "Granola authorization timed out before the localhost callback URL was entered.",
            )
        })?
        .map_err(|error| CliError::new("granola_oauth_callback_failed", error))
}

fn granola_headless_prompt_timeout() -> std::time::Duration {
    std::env::var("MARGINS_GRANOLA_OAUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(300))
        .min(std::time::Duration::from_secs(900))
}

fn granola_headless_consent_instructions(url: &str) -> String {
    format!(
        "Headless Granola consent: open this URL in your local browser.\n\
         The browser will intentionally finish on a localhost \"cannot connect\" page because no listener exists on the browser machine.\n\
         Copy the entire final address-bar URL, not only the code, into the hidden prompt in this same terminal.\n\
         Never paste callback URLs, codes, or tokens into chat, shell history, logs, or stdout JSON.\n\
         {url}"
    )
}
pub fn google(
    margins_home: &Path,
    expected_account: Option<&str>,
    headless: bool,
    json_output: bool,
    credential: Option<&[u8]>,
    output: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let credential = credential.ok_or_else(|| {
        CliError::unavailable("google_connect_unavailable", OFFICIAL_BUILD_REQUIRED)
    })?;
    validate_credential(credential)?;
    stderr.flush().map_err(io_error)?;
    let (mode, presenter, backend): (
        GoogleOAuthMode,
        Arc<dyn GoogleOAuthPresenter>,
        GoogleCredentialBackendKind,
    ) = if headless {
        (
            google_oauth_mode_for_connect(headless),
            Arc::new(HeadlessPresenter),
            google_cli_credential_backend(),
        )
    } else {
        (
            google_oauth_mode_for_connect(headless),
            Arc::new(BrowserPresenter),
            google_cli_credential_backend(),
        )
    };
    let ready = connect_google_account(
        margins_home,
        expected_account,
        backend,
        credential,
        mode,
        presenter,
    )
    .map_err(native_error)?;
    if json_output {
        writeln!(
            output,
            "{}",
            json!({
                "schema_version": 1,
                "service": "google",
                "connected": true,
                "account": ready.account,
                "storage": ready.storage.as_str(),
                "access": ready.access,
                "read_only": ready.read_only
            })
        )
        .map_err(io_error)?;
    } else {
        writeln!(
            output,
            "Google connected: {} — Gmail, Calendar, Drive, Docs, Meet (read-only)",
            ready.account
        )
        .map_err(io_error)?;
    }
    Ok(())
}

struct BrowserPresenter;

impl GoogleOAuthPresenter for BrowserPresenter {
    fn present_authorization_url(&self, url: &str, _need_code: bool) -> Result<String, String> {
        open_browser(url).map_err(|error| error.message().to_string())?;
        Ok(String::new())
    }
}

struct HeadlessPresenter;

impl GoogleOAuthPresenter for HeadlessPresenter {
    fn present_authorization_url(&self, url: &str, need_code: bool) -> Result<String, String> {
        eprintln!("{}", headless_consent_instructions(url));
        if !need_code {
            return Ok(String::new());
        }
        let callback =
            prompt_headless_callback().map_err(|error| redact_secret_text(&error.to_string()))?;
        validate_headless_callback_input(&callback)
    }
}

fn prompt_headless_callback() -> Result<String, CliError> {
    use std::sync::mpsc;
    use std::thread;

    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = rpassword::prompt_password("Final localhost address-bar URL: ")
            .map_err(|error| error.to_string());
        let _ = tx.send(result);
    });
    rx.recv_timeout(headless_prompt_timeout())
        .map_err(|_| {
            CliError::new(
                "google_oauth_timed_out",
                "Google authorization timed out before the localhost callback URL was entered.",
            )
        })?
        .map_err(|error| CliError::new("google_oauth_failed", error))
}

fn google_oauth_mode_for_connect(headless: bool) -> GoogleOAuthMode {
    if headless {
        GoogleOAuthMode::HeadlessLoopback {
            redirect_uri: String::new(),
        }
    } else {
        GoogleOAuthMode::BrowserLoopback
    }
}

fn google_cli_credential_backend() -> GoogleCredentialBackendKind {
    GoogleCredentialBackendKind::File0600
}

fn headless_prompt_timeout() -> std::time::Duration {
    std::env::var("MARGINS_GOOGLE_OAUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(std::time::Duration::from_secs(300))
        .min(std::time::Duration::from_secs(900))
}

fn validate_headless_callback_input(callback: &str) -> Result<String, String> {
    let callback = callback.trim();
    if callback.contains("://") && callback.contains('?') {
        Ok(callback.to_string())
    } else {
        Err(
            "Enter the entire final localhost address-bar URL, not only the authorization code."
                .to_string(),
        )
    }
}

fn headless_consent_instructions(url: &str) -> String {
    format!(
        "Headless Google consent: open this URL in your local browser.\n\
         The browser will intentionally finish on a localhost \"cannot connect\" page because no listener exists on the browser machine.\n\
         Copy the entire final address-bar URL, not only the code, into the hidden prompt in this same terminal.\n\
         Never paste callback URLs, codes, or tokens into chat, shell history, logs, or stdout JSON.\n\
         {url}"
    )
}

fn open_browser(url: &str) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).status();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open").arg(url).status();
    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", url]).status();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: std::io::Result<std::process::ExitStatus> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "browser launch unsupported",
    ));
    if result.is_ok_and(|status| status.success()) {
        Ok(())
    } else {
        Err(CliError::new(
            "google_browser_unavailable",
            "A browser could not be opened. Run `margins connect google` from a desktop session with a browser available.",
        ))
    }
}

pub fn status(
    margins_home: &Path,
    json_output: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    let mut names = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(margins_home.join("google")) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            if let Some(account) = entry.file_name().to_str() {
                if let Ok(account) = workspace::normalize_google_account(account) {
                    names.insert(account);
                }
            }
        }
    }
    for declared in workspace::list_workspaces(margins_home).map_err(CliError::from_anyhow)? {
        for account in declared
            .config
            .bindings
            .values()
            .filter_map(workspace::WorkspaceBinding::google_account)
        {
            if let Ok(account) = workspace::normalize_google_account(account) {
                names.insert(account);
            }
        }
    }
    let mut connections = Vec::new();
    for account in names {
        let store = GoogleAccountStore::new_with_backend(
            margins_home,
            &account,
            google_cli_credential_backend(),
        )
        .map_err(native_error)?;
        let durable = store.durable_connection_metadata(REQUIRED_GOOGLE_SCOPES);
        let (connected, metadata, reason, detail) = match durable {
            Ok(metadata) => (true, Some(metadata), None, None),
            Err(error) => {
                let native = error.downcast_ref::<GoogleNativeError>();
                let reason = native
                    .map(GoogleNativeError::code)
                    .unwrap_or("google_connection_needs_attention");
                (
                    false,
                    store.metadata().ok().flatten(),
                    Some(reason),
                    Some(
                        native
                            .map(native_user_message)
                            .unwrap_or_else(|| redact_secret_text(&format!("{error:#}"))),
                    ),
                )
            }
        };
        let workspaces = workspace::workspaces_using_google_account(margins_home, &account)
            .map_err(CliError::from_anyhow)?;
        connections.push(json!({
            "account": account,
            "connected": connected,
            "storage": metadata.as_ref().map(|metadata| metadata.storage.as_str()),
            "workspaces": workspaces,
            "status": if connected { "connected" } else { "needs_attention" },
            "reason": reason,
            "detail": detail,
            "access": if connected { json!(["gmail", "calendar", "drive", "docs", "meet"]) } else { json!([]) },
            "read_only": true
        }));
    }
    if json_output {
        writeln!(output, "{}", json!({"schema_version":1,"service":"google","scope":"machine","connections":connections})).map_err(io_error)?;
    } else if connections.is_empty() {
        writeln!(
            output,
            "Google not connected. Run `margins connect google`."
        )
        .map_err(io_error)?;
    } else {
        for connection in connections {
            let account = connection["account"].as_str().unwrap_or_default();
            let state = if connection["connected"].as_bool() == Some(true) {
                "connected"
            } else {
                "needs attention"
            };
            let users = connection["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            let usage = if users.is_empty() {
                "no Workspaces".to_string()
            } else {
                format!("Workspaces: {}", users.join(", "))
            };
            writeln!(output, "Google {state}: {account} — {usage}").map_err(io_error)?;
        }
    }
    Ok(())
}

pub fn granola_status(
    margins_home: &Path,
    json_output: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    granola_status_with(margins_home, json_output, output, |store| {
        store.durable_connection_metadata()
    })
}

fn granola_status_with(
    margins_home: &Path,
    json_output: bool,
    output: &mut dyn Write,
    mut validate: impl FnMut(&GranolaAccountStore) -> anyhow::Result<GranolaConnectionMetadata>,
) -> Result<(), CliError> {
    let mut names = list_granola_accounts(margins_home).map_err(CliError::from_anyhow)?;
    names.sort();
    names.dedup();
    let mut connections = Vec::new();
    for account in names {
        let store = GranolaAccountStore::new_with_backend(
            margins_home,
            &account,
            granola_cli_credential_backend(),
        )
        .map_err(granola_native_error)?;
        let usable = validate(&store);
        let (authorized, metadata, code, stage, reason, retryable, detail) = match usable {
            Ok(metadata) => (true, Some(metadata), None, None, None, false, None),
            Err(error) => {
                let failure = granola_failure_info(&error);
                let detail = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<GranolaNativeError>())
                    .map(granola_user_message);
                (
                    false,
                    store.metadata().ok().flatten(),
                    Some(
                        failure
                            .map(|failure| failure.code)
                            .unwrap_or("granola_connection_needs_attention"),
                    ),
                    failure.map(|failure| failure.stage),
                    failure.map(|failure| failure.reason),
                    failure.is_some_and(|failure| failure.retryable),
                    detail,
                )
            }
        };
        connections.push(json!({
            "account": account,
            "authorized": authorized,
            "storage": metadata.as_ref().map(|metadata| metadata.storage.as_str()),
            "status": if authorized { "authorized" } else { "needs_attention" },
            "code": code,
            "stage": stage,
            "reason": reason,
            "retryable": retryable,
            "detail": detail,
            "access": if authorized { json!(["meetings", "participants", "notes", "transcripts"]) } else { json!([]) },
        }));
    }
    if json_output {
        writeln!(
            output,
            "{}",
            json!({
                "schema_version": 1,
                "service": "granola",
                "scope": "machine",
                "connections": connections,
            })
        )
        .map_err(granola_io_error)?;
    } else if connections.is_empty() {
        writeln!(
            output,
            "Granola import is not authorized. Run `margins connect granola`."
        )
        .map_err(granola_io_error)?;
    } else {
        for connection in connections {
            let account = connection["account"].as_str().unwrap_or_default();
            let state = if connection["authorized"].as_bool() == Some(true) {
                "authorized"
            } else {
                "needs attention"
            };
            writeln!(output, "Granola {state}: {account}").map_err(granola_io_error)?;
        }
    }
    Ok(())
}

pub fn forget(
    margins_home: &Path,
    account: &str,
    json_output: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    let account = workspace::normalize_google_account(account).map_err(CliError::from_anyhow)?;
    let mut retained_workspaces =
        workspace::workspaces_using_google_account(margins_home, &account)
            .map_err(CliError::from_anyhow)?;
    retained_workspaces.sort();
    let home =
        workspace::google_account_dir(margins_home, &account).map_err(CliError::from_anyhow)?;
    if home.exists() {
        GoogleAccountStore::new_with_backend(
            margins_home,
            &account,
            google_cli_credential_backend(),
        )
        .and_then(|store| store.forget())
        .map_err(native_error)?;
    }
    mark_retained_workspaces_needs_auth(margins_home, &account, &retained_workspaces)?;
    if json_output {
        writeln!(output, "{}", json!({"schema_version":1,"service":"google","scope":"machine","account":account,"forgotten":true,"retained_workspaces":retained_workspaces})).map_err(io_error)?;
    } else {
        writeln!(output, "Google machine connection forgotten: {account}").map_err(io_error)?;
    }
    Ok(())
}

pub fn forget_granola(
    margins_home: &Path,
    account: &str,
    json_output: bool,
    output: &mut dyn Write,
) -> Result<(), CliError> {
    let account = workspace::normalize_granola_account(account).map_err(CliError::from_anyhow)?;
    let store = GranolaAccountStore::new_with_backend(
        margins_home,
        &account,
        granola_cli_credential_backend(),
    )
    .map_err(granola_native_error)?;
    store.forget().map_err(granola_native_error)?;
    if json_output {
        writeln!(
            output,
            "{}",
            json!({
                "schema_version": 1,
                "service": "granola",
                "scope": "machine",
                "account": account,
                "forgotten": true,
            })
        )
        .map_err(granola_io_error)?;
    } else {
        writeln!(output, "Granola machine connection forgotten: {account}")
            .map_err(granola_io_error)?;
    }
    Ok(())
}

fn granola_native_error(error: anyhow::Error) -> CliError {
    for cause in error.chain() {
        if let Some(native) = cause.downcast_ref::<GranolaNativeError>() {
            let mut error = CliError::new(native.code(), granola_user_message(native));
            if let Some((stage, reason)) = native.stage_reason() {
                let retryable = native.retryable();
                error = error
                    .with_details(json!({
                        "stage": stage,
                        "reason": reason,
                        "next_action": if retryable {
                            "run_connect_granola_again"
                        } else {
                            "update_margins_or_contact_granola"
                        }
                    }))
                    .retryable(retryable);
            }
            return error;
        }
    }
    let failure = granola_failure_info(&error);
    debug_assert!(failure.is_none());
    CliError::new(
        "granola_connect_failed",
        "Granola connection could not be updated. Run `margins connect granola` again.",
    )
}

fn granola_user_message(error: &GranolaNativeError) -> String {
    match error {
        GranolaNativeError::AccountMismatch { .. } =>
            "The OAuth identity and authenticated Granola account did not match. Reconnect and choose the intended Granola account."
                .to_string(),
        GranolaNativeError::StoragePermission { .. } => {
            "Granola credential file storage has unsafe permissions. Restrict it to owner read/write, then reconnect.".to_string()
        }
        GranolaNativeError::CredentialBackendMismatch { .. } => {
            "This is a legacy or desktop-managed Granola Keychain connection. One account cannot be owned by both Keychain and the CLI because its metadata records a single backend. On macOS, use Keychain Access to delete the matching `margins.granola` item, then move the non-secret account metadata directory at `$MARGINS_HOME/granola/<account>/` aside as a backup before running `margins connect granola` again. The CLI did not access or modify its credentials.".to_string()
        }
        GranolaNativeError::CredentialsUnavailable(_) => {
            "The CLI's private Granola credential file is unavailable. Run `margins connect granola` again; use `--headless` only when browser presentation requires it.".to_string()
        }
        GranolaNativeError::BrowserUnavailable => {
            "A browser could not be opened. Run `margins connect granola --headless` to use the private callback prompt.".to_string()
        }
        GranolaNativeError::OAuthTimedOut => {
            "Granola authorization timed out. Run `margins connect granola` again.".to_string()
        }
        GranolaNativeError::OAuthStage { stage, reason } => format!(
            "Granola authorization did not complete at {stage} ({reason}). Run `margins connect granola` again."
        ),
        GranolaNativeError::Mcp { .. } => {
            "Granola access could not be verified. Run `margins connect granola` again.".to_string()
        }
    }
}

fn granola_io_error(error: impl std::fmt::Display) -> CliError {
    let _ = error;
    CliError::new("output_failed", "Granola output could not be written.")
}

fn native_error(error: anyhow::Error) -> CliError {
    for cause in error.chain() {
        if let Some(native) = cause.downcast_ref::<GoogleNativeError>() {
            return CliError::new(native.code(), native_user_message(native));
        }
    }
    CliError::new(
        "google_connect_failed",
        format!(
            "Google connection could not be updated: {}. Run `margins connect google` again.",
            redact_secret_text(&format!("{error:#}"))
        ),
    )
}

fn native_user_message(error: &GoogleNativeError) -> String {
    match error {
        GoogleNativeError::AccountMismatch { expected, actual } => format!(
            "Google approved {actual}, but Margins expected {expected}. Run `margins connect google --account {expected}` again and choose that account."
        ),
        GoogleNativeError::ScopeMismatch { .. } => {
            "Google did not grant every required read-only Workspace permission. Run `margins connect google` again and approve Gmail, Calendar, Drive, Docs, and Meet access.".to_string()
        }
        GoogleNativeError::StoragePermission { .. } => {
            "Google credential file storage has unsafe permissions. Restrict it to owner read/write, then run `margins connect google` again.".to_string()
        }
        GoogleNativeError::CredentialBackendMismatch { .. } => {
            "This is a legacy or desktop-managed Google Keychain connection. One account cannot be owned by both Keychain and the CLI because its metadata records a single backend. On macOS, use Keychain Access to delete the matching `margins.google` item, then move the non-secret account metadata directory at `$MARGINS_HOME/google/<account>/` aside as a backup before running `margins connect google` again. The CLI did not access or modify its credentials.".to_string()
        }
        GoogleNativeError::CredentialsUnavailable(_) => {
            "The CLI's private Google credential file is unavailable. Run `margins connect google` again; use `--headless` only when browser presentation requires it.".to_string()
        }
        GoogleNativeError::Quota { .. } => {
            "Google temporarily rejected requests because of quota or rate limits. Wait and run the command again.".to_string()
        }
        GoogleNativeError::Http { .. } => {
            "Google access could not be verified. Run `margins connect google` again.".to_string()
        }
        GoogleNativeError::OAuth(_) => {
            "Google authorization failed. Run `margins connect google` again from a desktop browser session, or use `margins connect google --headless` from an isolated headless terminal.".to_string()
        }
    }
}

fn mark_retained_workspaces_needs_auth(
    margins_home: &Path,
    account: &str,
    retained_workspaces: &[String],
) -> Result<(), CliError> {
    for id in retained_workspaces {
        let workspace = workspace::resolve_at(margins_home, id).map_err(CliError::from_anyhow)?;
        if workspace.ledger_path().is_file() {
            IntegrationsStore::open(&workspace.state_dir)
                .and_then(|store| store.mark_google_connection_needs_auth(account).map(|_| ()))
                .map_err(CliError::from_anyhow)?;
        }
    }
    Ok(())
}

fn validate_credential(bytes: &[u8]) -> Result<(), CliError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| CliError::new("google_credential_invalid", OFFICIAL_BUILD_REQUIRED))?;
    let installed = value
        .get("installed")
        .and_then(Value::as_object)
        .ok_or_else(|| CliError::new("google_credential_invalid", OFFICIAL_BUILD_REQUIRED))?;
    for key in [
        "client_id",
        "client_secret",
        "auth_uri",
        "token_uri",
        "redirect_uris",
    ] {
        if !installed.contains_key(key) {
            return Err(CliError::new(
                "google_credential_invalid",
                OFFICIAL_BUILD_REQUIRED,
            ));
        }
    }
    if installed.values().any(|value| {
        value
            .as_str()
            .is_some_and(|text| text.starts_with("REPLACE_WITH_"))
    }) {
        return Err(CliError::new(
            "google_credential_invalid",
            OFFICIAL_BUILD_REQUIRED,
        ));
    }
    Ok(())
}

fn io_error(error: impl std::fmt::Display) -> CliError {
    CliError::new(
        "google_connect_failed",
        format!(
            "Google connection could not be updated: {error}. Run `margins connect google` again."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_granola_status_fixture(home: &Path, expires_at: chrono::DateTime<chrono::Utc>) {
        let account_dir = home.join("granola/owner@example.com");
        std::fs::create_dir_all(&account_dir).unwrap();
        std::fs::write(
            account_dir.join("account.json"),
            serde_json::to_vec(&json!({
                "schema_version": "margins.granola-account.v1",
                "account": "owner@example.com",
                "storage": "file_0600",
                "client_id": "fixture-client",
                "scopes": ["openid", "profile", "email", "offline_access"],
                "connected_at": "2026-08-20T17:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            account_dir.join("token-cache.json"),
            serde_json::to_vec(&json!({
                "schema_version": "margins.granola-token-cache.v1",
                "account": "owner@example.com",
                "client_id": "fixture-client",
                "access_token": "fixture-access",
                "refresh_token": "fixture-refresh",
                "expires_at": expires_at.to_rfc3339()
            }))
            .unwrap(),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                account_dir.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            std::fs::set_permissions(&account_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            for name in ["account.json", "token-cache.json"] {
                std::fs::set_permissions(
                    account_dir.join(name),
                    std::fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn fixture_has_the_installed_desktop_client_shape() {
        validate_credential(include_bytes!(
            "../../tests/fixtures/google-oauth-client.json"
        ))
        .unwrap();
    }
    #[test]
    fn user_facing_strings_hide_implementation() {
        let forbidden = ["go", "g"].concat();
        for message in [
            OFFICIAL_BUILD_REQUIRED,
            "Setting up Google access…",
            "Google not connected. Run `margins connect google`.",
            "Google access expired. Run `margins connect google` again to reconnect.",
        ] {
            assert!(!message.to_ascii_lowercase().contains(&forbidden));
        }
    }

    #[test]
    fn headless_consent_instructions_distinguish_full_callback_url_from_code_only() {
        let message = headless_consent_instructions("https://accounts.example/consent");
        assert!(message.contains("intentionally finish on a localhost \"cannot connect\" page"));
        assert!(message.contains("Copy the entire final address-bar URL, not only the code"));
        assert!(message.contains("hidden prompt in this same terminal"));
        assert!(message.contains("Never paste callback URLs, codes, or tokens into chat"));
        assert!(message.contains("https://accounts.example/consent"));
        let granola = granola_headless_consent_instructions("https://accounts.example/granola");
        assert!(granola.contains("Headless Granola consent"));
        assert!(granola.contains("entire final address-bar URL"));
        assert!(granola.contains("hidden prompt in this same terminal"));
        assert!(granola.contains("Never paste callback URLs, codes, or tokens into chat"));
    }

    #[test]
    fn headless_callback_input_requires_full_url_not_code_only() {
        let error = validate_headless_callback_input("4/0AdCodeOnly").unwrap_err();
        assert!(error.contains("entire final localhost address-bar URL"));
        assert!(error.contains("not only the authorization code"));
        let url = "http://127.0.0.1:49152/oauth2/callback?code=ok&state=good";
        assert_eq!(validate_headless_callback_input(url).unwrap(), url);
    }

    #[test]
    fn granola_token_exchange_error_is_typed_retryable_and_redacted() {
        let error = granola_native_error(anyhow::Error::new(GranolaNativeError::OAuthStage {
            stage: "browser_token_exchange",
            reason: "transport",
        }));

        assert_eq!(error.code(), "granola_oauth_token_exchange_failed");
        assert!(error.retryable_value());
        assert_eq!(
            error.details(),
            Some(&json!({
                "stage": "browser_token_exchange",
                "reason": "transport",
                "next_action": "run_connect_granola_again"
            }))
        );
        assert!(!error.message().contains("provider-private"));
        assert!(!error.message().contains("provider.invalid"));
    }

    #[test]
    fn granola_invalid_registration_metadata_is_typed_and_not_retryable() {
        let error = granola_native_error(anyhow::Error::new(GranolaNativeError::OAuthStage {
            stage: "client_registration",
            reason: "invalid_client_metadata",
        }));

        assert_eq!(error.code(), "granola_oauth_client_registration_failed");
        assert!(!error.retryable_value());
        assert_eq!(
            error.details(),
            Some(&json!({
                "stage": "client_registration",
                "reason": "invalid_client_metadata",
                "next_action": "update_margins_or_contact_granola"
            }))
        );
    }

    #[test]
    fn cli_storage_is_file_owned_in_browser_and_headless_modes() {
        assert_eq!(
            google_cli_credential_backend(),
            GoogleCredentialBackendKind::File0600
        );
        assert_eq!(
            granola_cli_credential_backend(),
            GranolaCredentialBackendKind::File0600
        );
        assert!(matches!(
            google_oauth_mode_for_connect(false),
            GoogleOAuthMode::BrowserLoopback
        ));
        assert!(matches!(
            google_oauth_mode_for_connect(true),
            GoogleOAuthMode::HeadlessLoopback { .. }
        ));
        assert!(matches!(
            granola_oauth_mode_for_connect(true),
            GranolaOAuthMode::HeadlessLoopback { .. }
        ));
    }

    #[test]
    fn backend_mismatch_copy_is_honest_and_names_manual_recovery() {
        let google = native_user_message(&GoogleNativeError::CredentialBackendMismatch {
            requested: GoogleCredentialBackendKind::File0600,
            persisted: GoogleCredentialBackendKind::OsKeyring,
        });
        let granola = granola_user_message(&GranolaNativeError::CredentialBackendMismatch {
            requested: GranolaCredentialBackendKind::File0600,
            persisted: GranolaCredentialBackendKind::OsKeyring,
        });
        for (message, service, path) in [
            (google, "margins.google", "$MARGINS_HOME/google/<account>/"),
            (
                granola,
                "margins.granola",
                "$MARGINS_HOME/granola/<account>/",
            ),
        ] {
            assert!(message.contains("legacy or desktop-managed"));
            assert!(message.contains("cannot be owned by both Keychain and the CLI"));
            assert!(message.contains("Keychain Access"));
            assert!(message.contains(service));
            assert!(message.contains(path));
            assert!(message.contains("move the non-secret account metadata directory"));
            assert!(message.contains("aside as a backup"));
            assert!(message.contains("CLI did not access or modify"));
            assert!(!message.contains("belongs to the desktop app"));
            assert!(!message.contains("Open Margins desktop"));
        }
    }

    #[test]
    fn backend_mismatch_status_includes_safe_manual_recovery() {
        let google_home = tempfile::tempdir().unwrap();
        let google_account = google_home.path().join("google/owner@example.com");
        std::fs::create_dir_all(&google_account).unwrap();
        std::fs::write(
            google_account.join("account.json"),
            serde_json::to_vec(&json!({
                "schema_version": "margins.google-account.v1",
                "account": "owner@example.com",
                "storage": "os_keyring",
                "scopes": REQUIRED_GOOGLE_SCOPES,
                "connected_at": "2026-08-20T17:00:00Z"
            }))
            .unwrap(),
        )
        .unwrap();
        let mut google_output = Vec::new();
        status(google_home.path(), true, &mut google_output).unwrap();
        let google: Value = serde_json::from_slice(&google_output).unwrap();

        let granola_home = tempfile::tempdir().unwrap();
        write_granola_status_fixture(
            granola_home.path(),
            chrono::Utc::now() + chrono::Duration::hours(1),
        );
        let granola_metadata = granola_home
            .path()
            .join("granola/owner@example.com/account.json");
        let mut value: Value =
            serde_json::from_slice(&std::fs::read(&granola_metadata).unwrap()).unwrap();
        value["storage"] = Value::String("os_keyring".into());
        std::fs::write(&granola_metadata, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut granola_output = Vec::new();
        granola_status(granola_home.path(), true, &mut granola_output).unwrap();
        let granola: Value = serde_json::from_slice(&granola_output).unwrap();

        for (row, code) in [
            (
                &google["connections"][0],
                "google_credential_backend_mismatch",
            ),
            (
                &granola["connections"][0],
                "granola_credential_backend_mismatch",
            ),
        ] {
            assert_eq!(
                row[if code.starts_with("google") {
                    "reason"
                } else {
                    "code"
                }],
                code
            );
            let detail = row["detail"].as_str().unwrap();
            assert!(detail.contains("legacy or desktop-managed"));
            assert!(detail.contains("Keychain Access"));
            assert!(detail.contains("$MARGINS_HOME/"));
            assert!(detail.contains("<account>"));
            assert!(detail.contains("CLI did not access or modify"));
            assert!(!detail.contains("owner@example.com"));
        }
    }

    #[test]
    fn granola_status_unexpired_token_is_authorized_without_network() {
        let temp = tempfile::tempdir().unwrap();
        write_granola_status_fixture(temp.path(), chrono::Utc::now() + chrono::Duration::hours(1));
        let mut output = Vec::new();
        granola_status(temp.path(), true, &mut output).unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["connections"][0]["authorized"], true);
        assert_eq!(value["connections"][0]["retryable"], false);
    }

    #[test]
    fn granola_status_expired_token_is_read_only() {
        let temp = tempfile::tempdir().unwrap();
        write_granola_status_fixture(temp.path(), chrono::Utc::now() - chrono::Duration::hours(1));
        let account = temp.path().join("granola/owner@example.com");
        let token_before = std::fs::read(account.join("token-cache.json")).unwrap();
        let metadata_before = std::fs::read(account.join("account.json")).unwrap();
        let mut output = Vec::new();
        granola_status(temp.path(), true, &mut output).unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["connections"][0]["authorized"], true);
        assert_eq!(value["connections"][0]["status"], "authorized");
        assert_eq!(
            std::fs::read(account.join("token-cache.json")).unwrap(),
            token_before
        );
        assert_eq!(
            std::fs::read(account.join("account.json")).unwrap(),
            metadata_before
        );
    }

    fn add_work_google_sources(workspace: &mut workspace::ResolvedWorkspace, account: &str) {
        for (name, binding) in [
            (
                "work-mail",
                workspace::WorkspaceBinding::Gmail {
                    account: account.to_string(),
                    gmail: workspace::GmailCollectionSelector::default_declaration(),
                },
            ),
            (
                "work-calendar",
                workspace::WorkspaceBinding::GoogleCalendar {
                    account: account.to_string(),
                    calendar: workspace::CalendarCollectionSelector::default_declaration(),
                },
            ),
            (
                "work-meet",
                workspace::WorkspaceBinding::GoogleMeet {
                    account: account.to_string(),
                },
            ),
        ] {
            workspace::add_source(workspace, name, binding).unwrap();
        }
    }

    #[test]
    fn forget_preserves_declarations_and_marks_retained_snapshots_needs_auth() {
        let temp = tempfile::tempdir().unwrap();
        for id in ["alpha", "beta"] {
            let notes = temp.path().join(format!("notes-{id}"));
            std::fs::create_dir_all(&notes).unwrap();
            let mut declared = workspace::create_workspace(temp.path(), id, None, &notes).unwrap();
            add_work_google_sources(&mut declared, "owner@example.com");
            let store = IntegrationsStore::open(&declared.state_dir).unwrap();
            for connector_id in ["email", "gcal", "google_meet"] {
                store
                    .update_health(
                        &margins_workflows::integrations::ConnectorCtx {
                            vault_root: declared.state_dir.clone(),
                            connector_id: connector_id.to_string(),
                            account: "owner@example.com".to_string(),
                            command_path: None,
                        },
                        margins_workflows::integrations::HealthStatus::Fresh,
                        Some("fixture refresh"),
                    )
                    .unwrap();
            }
            std::fs::write(declared.recall_path(), b"retained index").unwrap();
        }
        let mut output = Vec::new();
        forget(temp.path(), "owner@example.com", true, &mut output).unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["forgotten"], true);
        assert_eq!(value["account"], "owner@example.com");
        assert_eq!(value["retained_workspaces"], json!(["alpha", "beta"]));
        assert!(value.get("detached_workspaces").is_none());
        for id in ["alpha", "beta"] {
            let declared = workspace::resolve_at(temp.path(), id).unwrap();
            assert!(declared.config.bindings.contains_key("work-mail"));
            assert!(declared.config.bindings.contains_key("work-calendar"));
            assert!(declared.config.bindings.contains_key("work-meet"));
            assert_eq!(
                std::fs::read(declared.recall_path()).unwrap(),
                b"retained index"
            );
            let store = IntegrationsStore::open(&declared.state_dir).unwrap();
            let health = store
                .health_report(&margins_workflows::integrations::ConnectorCtx {
                    vault_root: declared.state_dir.clone(),
                    connector_id: "email".to_string(),
                    account: "owner@example.com".to_string(),
                    command_path: None,
                })
                .unwrap();
            assert_eq!(
                health.status,
                margins_workflows::integrations::HealthStatus::NeedsAuth
            );
            assert!(health.last_successful_sync.is_some());
        }
    }

    #[test]
    fn forget_granola_removes_only_machine_authorization() {
        let temp = tempfile::tempdir().unwrap();
        write_granola_status_fixture(temp.path(), chrono::Utc::now() + chrono::Duration::hours(1));
        let mut output = Vec::new();
        forget_granola(temp.path(), "owner@example.com", true, &mut output).unwrap();
        let result: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(result["forgotten"], true);
        assert!(result.get("retained_workspaces").is_none());
        assert!(!temp.path().join("granola/owner@example.com").exists());
    }
}
