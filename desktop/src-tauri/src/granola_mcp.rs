//! Desktop adapter for the portable Granola machine connection and MCP driver.

use crate::granola_import::GranolaImportOptions;
use margins_workflows::integrations::{
    connect_granola_account, fetch_granola_import_batch, granola_failure_info,
    list_granola_accounts, GranolaAccountStore, GranolaCredentialBackendKind, GranolaOAuthMode,
    GranolaOAuthPresenter,
};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Serialize)]
pub(crate) struct GranolaImportStatus {
    pub(crate) authorized: bool,
    pub(crate) account: Option<String>,
    pub(crate) accounts: Vec<String>,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'static str>,
    pub(crate) retryable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GranolaRemoteImportResult {
    pub(crate) imported_count: usize,
    pub(crate) note_paths: Vec<String>,
    pub(crate) people_created: usize,
    pub(crate) organizations_created: usize,
    pub(crate) warnings: Vec<String>,
    pub(crate) transcripts_plan_gated: bool,
}

#[derive(Clone, Copy)]
struct StatusFailure {
    code: &'static str,
    stage: &'static str,
    reason: &'static str,
    retryable: bool,
}

#[derive(Serialize)]
struct GranolaDesktopError {
    code: &'static str,
    stage: &'static str,
    reason: &'static str,
    retryable: bool,
}

pub(crate) fn typed_error(
    code: &'static str,
    stage: &'static str,
    reason: &'static str,
    retryable: bool,
) -> String {
    serde_json::to_string(&GranolaDesktopError {
        code,
        stage,
        reason,
        retryable,
    })
    .unwrap_or_else(|_| {
        "{\"code\":\"granola_internal_failed\",\"stage\":\"internal\",\"reason\":\"serialization_failed\",\"retryable\":false}".to_string()
    })
}

fn closed_error(error: anyhow::Error) -> String {
    granola_failure_info(&error)
        .map(|failure| {
            typed_error(
                failure.code,
                failure.stage,
                failure.reason,
                failure.retryable,
            )
        })
        .unwrap_or_else(|| typed_error("granola_internal_failed", "internal", "internal", false))
}

struct DesktopPresenter;

impl GranolaOAuthPresenter for DesktopPresenter {
    fn present_authorization_url(&self, url: &str, need_callback: bool) -> Result<String, String> {
        if need_callback {
            return Err("headless callback input is unavailable in the desktop app".to_string());
        }
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
            Ok(String::new())
        } else {
            Err("browser launch failed".to_string())
        }
    }
}

pub(crate) fn authorization_status() -> GranolaImportStatus {
    let home = match margins_workflows::workspace::margins_home() {
        Ok(home) => home,
        Err(_) => {
            return GranolaImportStatus {
                authorized: false,
                account: None,
                accounts: Vec::new(),
                message: "Granola connection state is unavailable.".to_string(),
                code: Some("granola_home_unavailable"),
                stage: Some("credential_validation"),
                reason: Some("home_unavailable"),
                retryable: false,
            }
        }
    };
    let accounts = list_granola_accounts(&home).unwrap_or_default();
    let mut authorized_accounts = Vec::new();
    let mut failure = None;
    for account in accounts {
        match GranolaAccountStore::new_with_backend(
            &home,
            &account,
            GranolaCredentialBackendKind::OsKeyring,
        )
        .and_then(|store| store.usable_connection_metadata())
        {
            Ok(metadata) => authorized_accounts.push(metadata.account),
            Err(error) => {
                failure.get_or_insert_with(|| status_failure(&error));
            }
        }
    }
    status_from_authorized_accounts(authorized_accounts, failure)
}

fn status_failure(error: &anyhow::Error) -> StatusFailure {
    granola_failure_info(error)
        .map(|failure| StatusFailure {
            code: failure.code,
            stage: failure.stage,
            reason: failure.reason,
            retryable: failure.retryable,
        })
        .unwrap_or(StatusFailure {
            code: "granola_internal_failed",
            stage: "internal",
            reason: "internal",
            retryable: false,
        })
}

fn status_from_authorized_accounts(
    authorized_accounts: Vec<String>,
    failure: Option<StatusFailure>,
) -> GranolaImportStatus {
    let code = failure.map(|failure| failure.code);
    let stage = failure.map(|failure| failure.stage);
    let reason = failure.map(|failure| failure.reason);
    let retryable = failure.is_some_and(|failure| failure.retryable);
    match authorized_accounts.as_slice() {
        [account] => GranolaImportStatus {
            authorized: true,
            account: Some(account.clone()),
            accounts: authorized_accounts.clone(),
            message: if failure.is_some() {
                "Granola import is authorized for one account; another authorization needs attention."
                    .to_string()
            } else {
                "Granola import is authorized on this machine.".to_string()
            },
            code,
            stage,
            reason,
            retryable,
        },
        [] => GranolaImportStatus {
            authorized: false,
            account: None,
            accounts: Vec::new(),
            message: if failure.is_some() {
                "Granola authorization needs attention. Authorize again.".to_string()
            } else {
                "Authorize Granola to import meetings into this project.".to_string()
            },
            code,
            stage,
            reason,
            retryable,
        },
        _ => GranolaImportStatus {
            authorized: true,
            account: None,
            accounts: authorized_accounts.clone(),
            message: if failure.is_some() {
                "Multiple Granola accounts are authorized; another authorization needs attention."
                    .to_string()
            } else {
                "Multiple Granola accounts are authorized; select an account before importing."
                    .to_string()
            },
            code,
            stage,
            reason,
            retryable,
        },
    }
}

pub(crate) fn resolve_authorized_account(
    status: &GranolaImportStatus,
    requested_account: Option<String>,
) -> Result<String, String> {
    if let Some(account) = requested_account {
        return Ok(account);
    }
    match status.accounts.as_slice() {
        [account] => Ok(account.clone()),
        [] => Err(typed_error(
            "granola_not_authorized",
            "authorization",
            "no_authorized_account",
            false,
        )),
        _ => Err(typed_error(
            "granola_account_ambiguous",
            "authorization",
            "multiple_authorized_accounts",
            false,
        )),
    }
}

pub(crate) fn authorize_blocking() -> Result<GranolaImportStatus, String> {
    let home = margins_workflows::workspace::margins_home().map_err(|_| {
        typed_error(
            "granola_home_unavailable",
            "credential_validation",
            "home_unavailable",
            false,
        )
    })?;
    connect_granola_account(
        &home,
        None,
        GranolaCredentialBackendKind::OsKeyring,
        GranolaOAuthMode::BrowserLoopback,
        &DesktopPresenter,
    )
    .map_err(closed_error)?;
    delete_legacy_desktop_refresh_token();
    Ok(authorization_status())
}

pub(crate) fn revoke_authorization(account: &str) -> Result<GranolaImportStatus, String> {
    let home = margins_workflows::workspace::margins_home().map_err(|_| {
        typed_error(
            "granola_home_unavailable",
            "credential_validation",
            "home_unavailable",
            false,
        )
    })?;
    GranolaAccountStore::new_with_backend(&home, account, GranolaCredentialBackendKind::OsKeyring)
        .and_then(|store| store.forget())
        .map_err(closed_error)?;
    delete_legacy_desktop_refresh_token();
    Ok(authorization_status())
}

fn delete_legacy_desktop_refresh_token() {
    let _ = crate::settings::keychain_delete_service(
        &crate::settings::keychain_service("granola-mcp"),
        "refresh-token",
    );
}

pub(crate) fn import_blocking(
    account: &str,
    vault_root: &Path,
    options: &GranolaImportOptions,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<GranolaRemoteImportResult, String> {
    let home = margins_workflows::workspace::margins_home().map_err(|_| {
        typed_error(
            "granola_home_unavailable",
            "credential_validation",
            "home_unavailable",
            false,
        )
    })?;
    let batch = fetch_granola_import_batch(
        &home,
        account,
        GranolaCredentialBackendKind::OsKeyring,
        progress,
    )
    .map_err(closed_error)?;
    let transcripts_plan_gated = batch.transcripts_plan_gated;
    let result = crate::granola_import::import_meetings(
        batch.meetings,
        batch.warnings,
        vault_root,
        options,
    )?;
    Ok(GranolaRemoteImportResult {
        imported_count: result.imported_count,
        note_paths: result.note_paths,
        people_created: result.people_created,
        organizations_created: result.organizations_created,
        warnings: result.warnings,
        transcripts_plan_gated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::integrations::GranolaNativeError;

    #[test]
    fn multi_account_status_lists_every_account_and_requires_explicit_selection() {
        let status = status_from_authorized_accounts(
            vec!["a@example.com".to_string(), "b@example.com".to_string()],
            None,
        );
        assert!(status.authorized);
        assert_eq!(status.account, None);
        assert_eq!(status.accounts.len(), 2);

        let error = resolve_authorized_account(&status, None).unwrap_err();
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "granola_account_ambiguous");
        assert_eq!(error["retryable"], false);
        assert_eq!(
            resolve_authorized_account(&status, Some("b@example.com".to_string())).unwrap(),
            "b@example.com"
        );
    }

    #[test]
    fn incoherent_accounts_do_not_claim_authorized() {
        let status = status_from_authorized_accounts(
            Vec::new(),
            Some(StatusFailure {
                code: "granola_credentials_unavailable",
                stage: "credential_validation",
                reason: "credentials_unavailable",
                retryable: false,
            }),
        );
        assert!(!status.authorized);
        assert!(status.account.is_none());
        assert!(status.accounts.is_empty());
        assert!(status.message.contains("needs attention"));
    }

    #[test]
    fn desktop_status_expired_valid_refresh_reports_authorized() {
        let status = status_from_authorized_accounts(vec!["owner@example.com".into()], None);
        assert!(status.authorized);
        assert_eq!(status.account.as_deref(), Some("owner@example.com"));
        assert!(status.code.is_none());
        assert!(!status.retryable);
    }

    #[test]
    fn desktop_status_expired_invalid_refresh_reports_typed_needs_auth() {
        let error = anyhow::Error::new(GranolaNativeError::OAuthStage {
            stage: "refresh",
            reason: "invalid_grant",
        });
        let status = status_from_authorized_accounts(Vec::new(), Some(status_failure(&error)));
        assert!(!status.authorized);
        assert_eq!(status.code, Some("granola_oauth_refresh_failed"));
        assert_eq!(status.stage, Some("refresh"));
        assert_eq!(status.reason, Some("invalid_grant"));
        assert!(!status.retryable);
    }

    #[test]
    fn desktop_status_expired_transport_failure_is_retryable_not_authorized() {
        let error = anyhow::Error::new(GranolaNativeError::OAuthStage {
            stage: "refresh",
            reason: "transport",
        });
        let status = status_from_authorized_accounts(Vec::new(), Some(status_failure(&error)));
        assert!(!status.authorized);
        assert_eq!(status.code, Some("granola_oauth_refresh_failed"));
        assert_eq!(status.reason, Some("transport"));
        assert!(status.retryable);
    }

    #[test]
    fn desktop_status_unexpired_no_network_path_remains_authorized() {
        let status = status_from_authorized_accounts(vec!["owner@example.com".into()], None);
        assert!(status.authorized);
        assert!(status.code.is_none());
        assert!(!status.retryable);
    }

    #[test]
    fn desktop_error_boundary_never_serializes_provider_details_and_retries_only_transients() {
        for (reason, retryable) in [
            ("transport", true),
            ("rate_limited", true),
            ("server_unavailable", true),
            ("unauthorized", false),
            ("rpc_rejected", false),
            ("invalid_json", false),
        ] {
            let error = anyhow::Error::new(GranolaNativeError::Mcp {
                code: "granola_mcp_http_failed",
                stage: "mcp_call",
                reason,
            })
            .context("SENTINEL https://sentinel.invalid/private?code=secret");
            let encoded = closed_error(error);
            assert!(!encoded.contains("SENTINEL"));
            assert!(!encoded.contains("sentinel.invalid"));
            let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded["stage"], "mcp_call");
            assert_eq!(decoded["reason"], reason);
            assert_eq!(decoded["retryable"], retryable);
        }

        let encoded = closed_error(anyhow::anyhow!(
            "SENTINEL https://sentinel.invalid/private?code=secret"
        ));
        assert!(!encoded.contains("SENTINEL"));
        assert!(!encoded.contains("sentinel.invalid"));
        let decoded: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded["code"], "granola_internal_failed");
        assert_eq!(decoded["retryable"], false);
    }
}
