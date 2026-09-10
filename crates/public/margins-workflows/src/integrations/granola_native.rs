//! Portable Granola OAuth, machine credential storage, and MCP transport.
//!
//! This module owns only the machine-level authorization capability and the
//! provider-shaped transport used to produce an ephemeral import batch.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthType, AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenUrl,
};
use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use url::Url;

use super::{
    ConnectorCtx, ExternalDocumentDelta, ExternalDocumentEvidence, ExternalDocumentParticipant,
    IntegrationsStore, RawItemDraft, ReconcileResult, SurveyRange,
};
use crate::granola_import::Meeting;
use crate::workspace::{
    normalize_granola_account, GranolaCollectionSelector, WorkspaceMutationError,
};

pub const GRANOLA_MCP_URL: &str = "https://mcp.granola.ai/mcp";
pub const GRANOLA_PROTECTED_RESOURCE_METADATA_URL: &str =
    "https://mcp.granola.ai/.well-known/oauth-protected-resource";
pub const GRANOLA_CONNECTOR_ID: &str = "granola";
const DEFAULT_AUTH_SERVER: &str = "https://mcp-auth.granola.ai";
const PROTOCOL_VERSION: &str = "2025-06-18";
const REQUIRED_SCOPES: &[&str] = &["openid", "profile", "email", "offline_access"];
const SERVICE_NAME: &str = "margins.granola";
const TOKEN_KEYRING_LABEL: &str = "oauth-token-cache";
const TOKEN_FILE_NAME: &str = "token-cache.json";
const METADATA_FILE_NAME: &str = "account.json";
const STORAGE_SCHEMA: &str = "margins.granola-token-cache.v1";
const METADATA_SCHEMA: &str = "margins.granola-account.v1";
const MAX_TOOLS_LIST_PAGES: usize = 32;
const MAX_DISCOVERED_TOOLS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GranolaCredentialBackendKind {
    #[serde(rename = "os_keyring")]
    OsKeyring,
    #[serde(rename = "file_0600")]
    File0600,
}

impl GranolaCredentialBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OsKeyring => "os_keyring",
            Self::File0600 => "file_0600",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GranolaNativeError {
    AccountMismatch {
        expected: String,
        actual: String,
    },
    StoragePermission {
        path: PathBuf,
        mode: u32,
    },
    CredentialBackendMismatch {
        requested: GranolaCredentialBackendKind,
        persisted: GranolaCredentialBackendKind,
    },
    CredentialsUnavailable(String),
    BrowserUnavailable,
    OAuthTimedOut,
    OAuthStage {
        stage: &'static str,
        reason: &'static str,
    },
    Mcp {
        code: &'static str,
        stage: &'static str,
        reason: &'static str,
    },
}

impl GranolaNativeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AccountMismatch { .. } => "granola_account_mismatch",
            Self::StoragePermission { .. } => "granola_storage_permission_insecure",
            Self::CredentialBackendMismatch { .. } => "granola_credential_backend_mismatch",
            Self::CredentialsUnavailable(_) => "granola_credentials_unavailable",
            Self::BrowserUnavailable => "granola_browser_unavailable",
            Self::OAuthTimedOut => "granola_oauth_timed_out",
            Self::OAuthStage { stage, .. } => match *stage {
                "client_registration" => "granola_oauth_client_registration_failed",
                "discovery" => "granola_oauth_discovery_failed",
                "browser_callback" => "granola_oauth_callback_failed",
                "browser_token_exchange" => "granola_oauth_token_exchange_failed",
                "token_response" => "granola_oauth_token_response_failed",
                "identity_verification" => "granola_oauth_identity_verification_failed",
                "refresh" => "granola_oauth_refresh_failed",
                _ => "granola_oauth_failed",
            },
            Self::Mcp { code, .. } => code,
        }
    }
}

impl std::fmt::Display for GranolaNativeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountMismatch { .. } => formatter.write_str("Granola account mismatch"),
            Self::StoragePermission { .. } => {
                formatter.write_str("Granola file credential store has insecure permissions")
            }
            Self::CredentialBackendMismatch {
                requested,
                persisted,
            } => write!(
                formatter,
                "Granola credential storage owner mismatch (requested {}, persisted {})",
                requested.as_str(),
                persisted.as_str()
            ),
            Self::CredentialsUnavailable(_) => {
                formatter.write_str("Granola credentials unavailable")
            }
            Self::BrowserUnavailable => {
                formatter.write_str("Granola sign-in browser could not be opened")
            }
            Self::OAuthTimedOut => formatter.write_str("Granola authorization timed out"),
            Self::OAuthStage { stage, reason } => write!(
                formatter,
                "Granola authorization failed at {stage}: {reason}"
            ),
            Self::Mcp { stage, reason, .. } => {
                write!(formatter, "Granola MCP failed at {stage}: {reason}")
            }
        }
    }
}

impl GranolaNativeError {
    pub fn stage_reason(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::OAuthStage { stage, reason } => Some((*stage, *reason)),
            Self::OAuthTimedOut => Some(("browser_callback", "expired")),
            Self::AccountMismatch { .. } => Some(("account_verification", "identity_mismatch")),
            Self::CredentialsUnavailable(_) => {
                Some(("credential_validation", "credentials_unavailable"))
            }
            Self::BrowserUnavailable => Some(("browser_launch", "browser_unavailable")),
            Self::StoragePermission { .. } => Some(("credential_commit", "insecure_permissions")),
            Self::CredentialBackendMismatch { .. } => {
                Some(("credential_validation", "backend_mismatch"))
            }
            Self::Mcp { stage, reason, .. } => Some((*stage, *reason)),
        }
    }

    pub fn retryable(&self) -> bool {
        self.stage_reason().is_some_and(|(_, reason)| {
            matches!(reason, "transport" | "rate_limited" | "server_unavailable")
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GranolaFailureInfo {
    pub code: &'static str,
    pub stage: &'static str,
    pub reason: &'static str,
    pub retryable: bool,
}

pub fn granola_failure_info(error: &anyhow::Error) -> Option<GranolaFailureInfo> {
    error.chain().find_map(|cause| {
        let native = cause.downcast_ref::<GranolaNativeError>()?;
        let (stage, reason) = native.stage_reason()?;
        Some(GranolaFailureInfo {
            code: native.code(),
            stage,
            reason,
            retryable: native.retryable(),
        })
    })
}

impl std::error::Error for GranolaNativeError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GranolaConnectionMetadata {
    pub schema_version: String,
    pub account: String,
    pub storage: GranolaCredentialBackendKind,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub connected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GranolaConnectionReady {
    pub account: String,
    pub storage: GranolaCredentialBackendKind,
    pub scopes: Vec<String>,
    pub access: Vec<&'static str>,
}

#[derive(Debug, Clone)]
pub enum GranolaOAuthMode {
    BrowserLoopback,
    HeadlessLoopback { redirect_uri: String },
}

pub trait GranolaOAuthPresenter: Send + Sync {
    fn present_authorization_url(
        &self,
        url: &str,
        need_callback: bool,
    ) -> std::result::Result<String, String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredToken {
    schema_version: String,
    account: String,
    client_id: String,
    access_token: String,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
}

impl StoredToken {
    fn access_is_usable(&self) -> bool {
        !self.access_token.trim().is_empty()
            && self
                .expires_at
                .is_none_or(|expires| expires > Utc::now() + chrono::Duration::minutes(2))
    }

    fn is_refreshable(&self) -> bool {
        !self.client_id.trim().is_empty() && !self.refresh_token.trim().is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct GranolaAccountStore {
    account: String,
    account_dir: PathBuf,
    backend: GranolaCredentialBackendKind,
}

impl GranolaAccountStore {
    pub fn new(margins_home: &Path, account: &str) -> Result<Self> {
        Self::new_with_backend(margins_home, account, select_backend())
    }

    pub fn new_with_backend(
        margins_home: &Path,
        account: &str,
        backend: GranolaCredentialBackendKind,
    ) -> Result<Self> {
        let account = normalize_granola_account(account)?;
        Ok(Self {
            account_dir: margins_home.join("granola").join(&account),
            account,
            backend,
        })
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn account_dir(&self) -> &Path {
        &self.account_dir
    }

    pub fn backend(&self) -> GranolaCredentialBackendKind {
        self.backend.clone()
    }

    pub fn metadata(&self) -> Result<Option<GranolaConnectionMetadata>> {
        let path = self.metadata_path();
        if !path.exists() {
            return Ok(None);
        }
        if let Some(root) = self.account_dir.parent() {
            validate_private_dir_permissions(root)?;
        }
        validate_private_dir_permissions(&self.account_dir)?;
        validate_private_file_permissions(&path)?;
        let metadata: GranolaConnectionMetadata =
            serde_json::from_slice(&std::fs::read(&path).with_context(|| {
                format!("reading Granola connection metadata at {}", path.display())
            })?)
            .context("parsing Granola connection metadata")?;
        Ok(Some(metadata))
    }

    /// Check credential-store ownership using account metadata only.
    ///
    /// A historical `os_keyring` record does not say whether an older CLI or
    /// the desktop app created it. A File0600 caller must therefore refuse it:
    /// migrating it would require crossing the Keychain boundary that this
    /// preflight exists to protect.
    pub fn preflight_backend_ownership(&self) -> Result<()> {
        if let Some(metadata) = self.metadata()? {
            self.ensure_backend_matches(&metadata)?;
        }
        Ok(())
    }

    pub fn durable_connection_metadata(&self) -> Result<GranolaConnectionMetadata> {
        let metadata = self.metadata()?.ok_or_else(|| {
            GranolaNativeError::CredentialsUnavailable("missing account metadata".into())
        })?;
        if metadata.account != self.account {
            return Err(GranolaNativeError::AccountMismatch {
                expected: self.account.clone(),
                actual: metadata.account,
            }
            .into());
        }
        self.ensure_backend_matches(&metadata)?;
        if metadata.schema_version != METADATA_SCHEMA {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "unsupported account metadata schema".into(),
            )
            .into());
        }
        if metadata.client_id.trim().is_empty() {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "missing OAuth client identity".into(),
            )
            .into());
        }
        if REQUIRED_SCOPES
            .iter()
            .any(|required| !metadata.scopes.iter().any(|scope| scope == required))
        {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "stored authorization is missing a required scope".into(),
            )
            .into());
        }
        let token = self.load_token()?.ok_or_else(|| {
            GranolaNativeError::CredentialsUnavailable("missing OAuth token cache".into())
        })?;
        if token.account != self.account || token.client_id != metadata.client_id {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "OAuth token cache does not match account metadata".into(),
            )
            .into());
        }
        if token.schema_version != STORAGE_SCHEMA {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "unsupported token cache schema".into(),
            )
            .into());
        }
        if !token.is_refreshable() {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "stored token has no refresh path for unattended access".into(),
            )
            .into());
        }
        Ok(metadata)
    }

    fn commit(
        &self,
        client_id: &str,
        scopes: &[String],
        token: StoredToken,
    ) -> Result<GranolaConnectionMetadata> {
        let previous_metadata = self.metadata()?;
        if let Some(metadata) = previous_metadata.as_ref() {
            self.ensure_backend_matches(metadata)?;
        }
        let previous_token = self.load_token()?;
        let previous_metadata_bytes = if self.metadata_path().exists() {
            Some(std::fs::read(self.metadata_path()).with_context(|| {
                format!(
                    "snapshotting Granola connection metadata at {}",
                    self.metadata_path().display()
                )
            })?)
        } else {
            None
        };
        if let Some(root) = self.account_dir.parent() {
            ensure_private_dir_chain(root)?;
        }
        ensure_private_dir_chain(&self.account_dir)?;
        if let Err(error) = self.store_token(&token) {
            self.rollback_commit(previous_token.as_ref(), previous_metadata_bytes.as_deref());
            return Err(error);
        }
        let metadata = GranolaConnectionMetadata {
            schema_version: METADATA_SCHEMA.to_string(),
            account: self.account.clone(),
            storage: self.backend.clone(),
            client_id: client_id.to_string(),
            scopes: scopes.to_vec(),
            connected_at: Utc::now(),
        };
        if let Err(error) = write_private_json(&self.metadata_path(), &metadata) {
            self.rollback_commit(previous_token.as_ref(), previous_metadata_bytes.as_deref());
            return Err(error);
        }
        if let Err(error) = self.durable_connection_metadata() {
            self.rollback_commit(previous_token.as_ref(), previous_metadata_bytes.as_deref());
            return Err(error);
        }
        Ok(metadata)
    }

    fn rollback_commit(
        &self,
        previous_token: Option<&StoredToken>,
        previous_metadata: Option<&[u8]>,
    ) {
        let _ = self.delete_token();
        if let Some(token) = previous_token {
            let _ = self.store_token(token);
        }
        if let Some(bytes) = previous_metadata {
            let _ = write_private_bytes(&self.metadata_path(), bytes);
        } else {
            let _ = std::fs::remove_file(self.metadata_path());
        }
    }

    pub fn forget(&self) -> Result<()> {
        if let Some(metadata) = self.metadata()? {
            self.ensure_backend_matches(&metadata)?;
        }
        self.delete_token()?;
        if self.account_dir.exists() {
            std::fs::remove_dir_all(&self.account_dir).with_context(|| {
                format!(
                    "removing Granola account directory {}",
                    self.account_dir.display()
                )
            })?;
        }
        Ok(())
    }

    pub fn ensure_access_token(&self) -> Result<String> {
        let metadata = self.durable_connection_metadata()?;
        self.ensure_access_token_for_metadata(&metadata, || Ok(discover_auth_endpoints()?.token))
    }

    /// Validate that this machine credential has an immediately usable access
    /// path. Unexpired credentials remain read-only; expired credentials make
    /// one bounded refresh attempt through the typed OAuth boundary.
    pub fn usable_connection_metadata(&self) -> Result<GranolaConnectionMetadata> {
        let metadata = self.durable_connection_metadata()?;
        self.ensure_access_token_for_metadata(&metadata, || Ok(discover_auth_endpoints()?.token))?;
        Ok(metadata)
    }

    fn ensure_access_token_for_metadata(
        &self,
        metadata: &GranolaConnectionMetadata,
        token_endpoint: impl FnOnce() -> Result<String>,
    ) -> Result<String> {
        self.ensure_backend_matches(metadata)?;
        let mut token = self.load_token()?.ok_or_else(|| {
            GranolaNativeError::CredentialsUnavailable("missing OAuth token cache".into())
        })?;
        if token.access_is_usable() {
            return Ok(token.access_token);
        }
        let token_endpoint = token_endpoint()?;
        let client = oauth_http_client().map_err(|_| GranolaNativeError::OAuthStage {
            stage: "refresh",
            reason: "client_unavailable",
        })?;
        let refreshed = exchange_token(
            &client,
            &token_endpoint,
            "refresh",
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", token.refresh_token.as_str()),
                ("client_id", metadata.client_id.as_str()),
                ("resource", GRANOLA_MCP_URL),
            ],
        );
        let refreshed = match refreshed {
            Ok(refreshed) => refreshed,
            Err(error) => {
                let invalidated = error
                    .downcast_ref::<GranolaNativeError>()
                    .and_then(GranolaNativeError::stage_reason)
                    .is_some_and(|(stage, reason)| {
                        stage == "refresh"
                            && matches!(
                                reason,
                                "invalid_grant" | "invalid_client" | "unauthorized_client"
                            )
                    });
                if invalidated {
                    let _ = self.forget();
                }
                return Err(error);
            }
        };
        let expires_at = refreshed.expires_at();
        token.access_token = refreshed.access_token;
        if let Some(refresh) = refreshed.refresh_token {
            token.refresh_token = refresh;
        }
        token.expires_at = expires_at;
        self.store_token(&token)?;
        Ok(token.access_token)
    }

    fn ensure_backend_matches(&self, metadata: &GranolaConnectionMetadata) -> Result<()> {
        if metadata.storage != self.backend {
            return Err(GranolaNativeError::CredentialBackendMismatch {
                requested: self.backend.clone(),
                persisted: metadata.storage.clone(),
            }
            .into());
        }
        Ok(())
    }

    fn metadata_path(&self) -> PathBuf {
        self.account_dir.join(METADATA_FILE_NAME)
    }

    fn token_path(&self) -> PathBuf {
        self.account_dir.join(TOKEN_FILE_NAME)
    }

    fn keyring_user(&self) -> String {
        format!("{}:{TOKEN_KEYRING_LABEL}", self.account)
    }

    fn store_token(&self, token: &StoredToken) -> Result<()> {
        let encoded = serde_json::to_string(token)?;
        match self.backend {
            GranolaCredentialBackendKind::OsKeyring => {
                keyring::Entry::new(SERVICE_NAME, &self.keyring_user())
                    .map_err(|error| GranolaNativeError::CredentialsUnavailable(error.to_string()))?
                    .set_password(&encoded)
                    .map_err(|error| {
                        GranolaNativeError::CredentialsUnavailable(error.to_string()).into()
                    })
            }
            GranolaCredentialBackendKind::File0600 => {
                write_private_bytes(&self.token_path(), encoded.as_bytes())
            }
        }
    }

    fn load_token(&self) -> Result<Option<StoredToken>> {
        let encoded = match self.backend {
            GranolaCredentialBackendKind::OsKeyring => {
                let entry =
                    keyring::Entry::new(SERVICE_NAME, &self.keyring_user()).map_err(|error| {
                        GranolaNativeError::CredentialsUnavailable(error.to_string())
                    })?;
                match entry.get_password() {
                    Ok(value) => value,
                    Err(keyring::Error::NoEntry) => return Ok(None),
                    Err(error) => {
                        return Err(
                            GranolaNativeError::CredentialsUnavailable(error.to_string()).into(),
                        );
                    }
                }
            }
            GranolaCredentialBackendKind::File0600 => {
                let path = self.token_path();
                if !path.exists() {
                    return Ok(None);
                }
                validate_private_file_permissions(&path)?;
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading Granola token cache at {}", path.display()))?
            }
        };
        serde_json::from_str(&encoded)
            .context("parsing Granola token cache")
            .map(Some)
    }

    fn delete_token(&self) -> Result<()> {
        match self.backend {
            GranolaCredentialBackendKind::OsKeyring => {
                let entry =
                    keyring::Entry::new(SERVICE_NAME, &self.keyring_user()).map_err(|error| {
                        GranolaNativeError::CredentialsUnavailable(error.to_string())
                    })?;
                match entry.delete_credential() {
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(error) => {
                        Err(GranolaNativeError::CredentialsUnavailable(error.to_string()).into())
                    }
                }
            }
            GranolaCredentialBackendKind::File0600 => {
                let path = self.token_path();
                if path.exists() {
                    validate_private_file_permissions(&path)?;
                    std::fs::remove_file(&path)?;
                }
                Ok(())
            }
        }
    }
}

fn select_backend() -> GranolaCredentialBackendKind {
    match std::env::var("MARGINS_GRANOLA_CREDENTIAL_BACKEND")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "file" | "file_0600" | "headless_file" => GranolaCredentialBackendKind::File0600,
        _ => GranolaCredentialBackendKind::OsKeyring,
    }
}

pub fn list_granola_accounts(margins_home: &Path) -> Result<Vec<String>> {
    let mut accounts = BTreeSet::new();
    let root = margins_home.join("granola");
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten().filter(|entry| entry.path().is_dir()) {
            if let Some(raw) = entry.file_name().to_str() {
                if let Ok(account) = normalize_granola_account(raw) {
                    accounts.insert(account);
                }
            }
        }
    }
    Ok(accounts.into_iter().collect())
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    authorization_servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    #[serde(default)]
    grant_types_supported: Vec<String>,
    #[serde(default)]
    response_types_supported: Vec<String>,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    token_endpoint_auth_methods_supported: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OpenIdMetadata {
    userinfo_endpoint: String,
}

#[derive(Debug, Clone)]
struct AuthEndpoints {
    authorize: String,
    token: String,
    register: String,
    userinfo: String,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

impl OAuthTokenResponse {
    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.expires_in
            .and_then(|seconds| Some(chrono::Duration::seconds(i64::try_from(seconds).ok()?)))
            .map(|duration| Utc::now() + duration)
    }
}

fn exchange_token(
    client: &Client,
    endpoint: &str,
    stage: &'static str,
    parameters: &[(&str, &str)],
) -> Result<OAuthTokenResponse> {
    let response = client.post(endpoint).form(parameters).send().map_err(|_| {
        GranolaNativeError::OAuthStage {
            stage,
            reason: "transport",
        }
    })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        let standard_error = response.json::<Value>().ok().and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        return Err(GranolaNativeError::OAuthStage {
            stage,
            reason: classify_oauth_http_failure(status, standard_error.as_deref()),
        }
        .into());
    }
    let token: OAuthTokenResponse =
        response
            .json()
            .map_err(|_| GranolaNativeError::OAuthStage {
                stage,
                reason: "invalid_response",
            })?;
    if token.access_token.trim().is_empty() {
        return Err(GranolaNativeError::OAuthStage {
            stage,
            reason: "invalid_response",
        }
        .into());
    }
    Ok(token)
}

fn classify_oauth_http_failure(status: u16, standard_error: Option<&str>) -> &'static str {
    if status == 429 {
        return "rate_limited";
    }
    if (500..=599).contains(&status) {
        return "server_unavailable";
    }
    match standard_error {
        Some("invalid_grant") => "invalid_grant",
        Some("invalid_client") => "invalid_client",
        Some("unauthorized_client") => "unauthorized_client",
        Some("invalid_request") => "invalid_request",
        Some("invalid_scope") => "invalid_scope",
        _ => "server_rejected",
    }
}

fn discover_auth_endpoints() -> Result<AuthEndpoints> {
    let client = discovery_http_client().map_err(|_| GranolaNativeError::OAuthStage {
        stage: "discovery",
        reason: "client_unavailable",
    })?;
    let protected: ProtectedResourceMetadata = fetch_oauth_json(
        &client,
        GRANOLA_PROTECTED_RESOURCE_METADATA_URL,
        "discovery",
    )?;
    let auth_server = protected
        .authorization_servers
        .first()
        .map(|server| server.trim_end_matches('/'))
        .unwrap_or(DEFAULT_AUTH_SERVER);
    require_https_endpoint(auth_server)?;
    let oauth: AuthorizationServerMetadata = fetch_oauth_json(
        &client,
        &format!("{auth_server}/.well-known/oauth-authorization-server"),
        "discovery",
    )?;
    for required in ["authorization_code", "refresh_token"] {
        if !oauth
            .grant_types_supported
            .iter()
            .any(|value| value == required)
        {
            bail!(GranolaNativeError::OAuthStage {
                stage: "discovery",
                reason: "unsupported_metadata",
            });
        }
    }
    if !oauth
        .response_types_supported
        .iter()
        .any(|value| value == "code")
        || !oauth
            .code_challenge_methods_supported
            .iter()
            .any(|value| value == "S256")
    {
        bail!(GranolaNativeError::OAuthStage {
            stage: "discovery",
            reason: "unsupported_metadata",
        });
    }
    if !oauth
        .token_endpoint_auth_methods_supported
        .iter()
        .any(|value| value == "none")
    {
        bail!(GranolaNativeError::OAuthStage {
            stage: "discovery",
            reason: "unsupported_metadata",
        });
    }
    if REQUIRED_SCOPES
        .iter()
        .any(|required| !oauth.scopes_supported.iter().any(|scope| scope == required))
    {
        bail!(GranolaNativeError::OAuthStage {
            stage: "discovery",
            reason: "unsupported_metadata",
        });
    }
    for endpoint in [
        &oauth.authorization_endpoint,
        &oauth.token_endpoint,
        &oauth.registration_endpoint,
    ] {
        require_https_endpoint(endpoint)?;
    }
    let openid: OpenIdMetadata = fetch_oauth_json(
        &client,
        &format!("{auth_server}/.well-known/openid-configuration"),
        "discovery",
    )?;
    require_https_endpoint(&openid.userinfo_endpoint)?;
    Ok(AuthEndpoints {
        authorize: oauth.authorization_endpoint,
        token: oauth.token_endpoint,
        register: oauth.registration_endpoint,
        userinfo: openid.userinfo_endpoint,
    })
}

fn fetch_oauth_json<T: DeserializeOwned>(
    client: &Client,
    endpoint: &str,
    stage: &'static str,
) -> Result<T> {
    let response = client
        .get(endpoint)
        .send()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage,
            reason: "transport",
        })?;
    let status = response.status().as_u16();
    if !response.status().is_success() {
        return Err(GranolaNativeError::OAuthStage {
            stage,
            reason: classify_oauth_http_failure(status, None),
        }
        .into());
    }
    response.json().map_err(|_| {
        GranolaNativeError::OAuthStage {
            stage,
            reason: "invalid_response",
        }
        .into()
    })
}

fn require_https_endpoint(endpoint: &str) -> Result<()> {
    let url = Url::parse(endpoint).map_err(|_| GranolaNativeError::OAuthStage {
        stage: "discovery",
        reason: "invalid_endpoint",
    })?;
    if url.scheme() != "https" || url.host_str().is_none() {
        bail!(GranolaNativeError::OAuthStage {
            stage: "discovery",
            reason: "invalid_endpoint",
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct DynamicClientRegistration {
    client_id: String,
}

fn register_client(endpoints: &AuthEndpoints, redirect_uri: &str) -> Result<String> {
    let body = registration_request(redirect_uri);
    let response = discovery_http_client()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "client_registration",
            reason: "client_unavailable",
        })?
        .post(&endpoints.register)
        .json(&body)
        .send()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "client_registration",
            reason: "transport",
        })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let standard_error = response.json::<Value>().ok().and_then(|value| {
            value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        return Err(GranolaNativeError::OAuthStage {
            stage: "client_registration",
            reason: classify_registration_failure(status, standard_error.as_deref()),
        }
        .into());
    }
    let registration: DynamicClientRegistration =
        response
            .json()
            .map_err(|_| GranolaNativeError::OAuthStage {
                stage: "client_registration",
                reason: "invalid_response",
            })?;
    if registration.client_id.trim().is_empty() {
        bail!(GranolaNativeError::OAuthStage {
            stage: "client_registration",
            reason: "missing_client_identity",
        });
    }
    Ok(registration.client_id)
}

fn registration_request(redirect_uri: &str) -> Value {
    json!({
        "client_name": "Margins",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "redirect_uris": [redirect_uri],
        "token_endpoint_auth_method": "none",
        "scope": REQUIRED_SCOPES.join(" "),
    })
}

fn classify_registration_failure(status: u16, standard_error: Option<&str>) -> &'static str {
    match standard_error {
        Some("invalid_redirect_uri") => "invalid_redirect_uri",
        Some("invalid_client_metadata") => "invalid_client_metadata",
        Some("invalid_software_statement") => "invalid_software_statement",
        Some("unapproved_software_statement") => "unapproved_software_statement",
        _ if status == 429 => "rate_limited",
        _ if (500..=599).contains(&status) => "server_unavailable",
        _ if matches!(status, 400 | 422) => "invalid_client_metadata",
        _ => "server_rejected",
    }
}

pub fn connect_granola_account(
    margins_home: &Path,
    expected_account: Option<&str>,
    backend: GranolaCredentialBackendKind,
    mode: GranolaOAuthMode,
    presenter: &dyn GranolaOAuthPresenter,
) -> Result<GranolaConnectionReady> {
    if let Some(account) = expected_account {
        GranolaAccountStore::new_with_backend(margins_home, account, backend.clone())?
            .preflight_backend_ownership()?;
    }
    let endpoints = discover_auth_endpoints()?;
    let client = oauth_http_client().map_err(|_| GranolaNativeError::OAuthStage {
        stage: "discovery",
        reason: "client_unavailable",
    })?;
    let (redirect_uri, listener) = match mode {
        GranolaOAuthMode::BrowserLoopback => {
            let listener =
                TcpListener::bind("127.0.0.1:0").map_err(|_| GranolaNativeError::OAuthStage {
                    stage: "browser_callback",
                    reason: "listener_unavailable",
                })?;
            (
                format!(
                    "http://127.0.0.1:{}/oauth2/callback",
                    listener
                        .local_addr()
                        .map_err(|_| GranolaNativeError::OAuthStage {
                            stage: "browser_callback",
                            reason: "listener_unavailable",
                        })?
                        .port()
                ),
                Some(listener),
            )
        }
        GranolaOAuthMode::HeadlessLoopback { redirect_uri } => {
            let redirect_uri = if redirect_uri.trim().is_empty() {
                ephemeral_loopback_redirect_uri()?
            } else {
                redirect_uri
            };
            (redirect_uri, None)
        }
    };
    let client_id = register_client(&endpoints, &redirect_uri)?;
    let oauth = BasicClient::new(ClientId::new(client_id.clone()))
        .set_auth_type(AuthType::RequestBody)
        .set_auth_uri(AuthUrl::new(endpoints.authorize.clone()).map_err(|_| {
            GranolaNativeError::OAuthStage {
                stage: "discovery",
                reason: "invalid_endpoint",
            }
        })?)
        .set_token_uri(TokenUrl::new(endpoints.token.clone()).map_err(|_| {
            GranolaNativeError::OAuthStage {
                stage: "discovery",
                reason: "invalid_endpoint",
            }
        })?)
        .set_redirect_uri(RedirectUrl::new(redirect_uri.clone()).map_err(|_| {
            GranolaNativeError::OAuthStage {
                stage: "browser_callback",
                reason: "invalid_redirect",
            }
        })?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = oauth.authorize_url(CsrfToken::new_random);
    for scope in REQUIRED_SCOPES {
        request = request.add_scope(Scope::new((*scope).to_string()));
    }
    let (authorization_url, state) = request
        .set_pkce_challenge(challenge)
        .add_extra_param("resource", GRANOLA_MCP_URL)
        .url();
    let code = match listener {
        Some(listener) => {
            presenter
                .present_authorization_url(authorization_url.as_str(), false)
                .map_err(|_| GranolaNativeError::BrowserUnavailable)?;
            wait_for_callback(&listener, state.secret(), Duration::from_secs(300))?
        }
        None => {
            let callback = presenter
                .present_authorization_url(authorization_url.as_str(), true)
                .map_err(|_| GranolaNativeError::OAuthStage {
                    stage: "browser_callback",
                    reason: "presentation_failed",
                })?;
            validate_callback_url(&callback, &redirect_uri, state.secret())?
        }
    };
    let token = exchange_token(
        &client,
        &endpoints.token,
        "browser_token_exchange",
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.secret()),
            ("resource", GRANOLA_MCP_URL),
        ],
    )?;
    let (client_id, access_token, refresh_token, expires_at, scopes) =
        token_parts(&client_id, &token)?;

    let account = verified_account(&endpoints.userinfo, &access_token)?;
    if let Some(expected) = expected_account {
        let expected = normalize_granola_account(expected)?;
        if expected != account {
            return Err(GranolaNativeError::AccountMismatch {
                expected,
                actual: account,
            }
            .into());
        }
    }
    let token = StoredToken {
        schema_version: STORAGE_SCHEMA.to_string(),
        account: account.clone(),
        client_id: client_id.clone(),
        access_token,
        refresh_token,
        expires_at,
    };
    commit_verified_connection(
        margins_home,
        &account,
        backend,
        &client_id,
        &scopes,
        token,
        GRANOLA_MCP_URL,
    )
}

fn commit_verified_connection(
    margins_home: &Path,
    account: &str,
    backend: GranolaCredentialBackendKind,
    client_id: &str,
    scopes: &[String],
    token: StoredToken,
    mcp_endpoint: &str,
) -> Result<GranolaConnectionReady> {
    let store = GranolaAccountStore::new_with_backend(margins_home, account, backend.clone())?;
    // With no expected account, the OAuth identity is unknowable until after
    // consent and userinfo. This is the earliest safe ownership check, and it
    // still runs before the MCP capability probe.
    store.preflight_backend_ownership()?;
    verify_mcp_access(&token.access_token, mcp_endpoint, account)?;
    store.commit(client_id, scopes, token)?;
    Ok(GranolaConnectionReady {
        account: account.to_string(),
        storage: backend,
        scopes: scopes.to_vec(),
        access: vec!["meetings", "participants", "notes", "transcripts"],
    })
}

fn token_parts(
    client_id: &str,
    token: &OAuthTokenResponse,
) -> Result<(String, String, String, Option<DateTime<Utc>>, Vec<String>)> {
    let refresh = token
        .refresh_token
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or(GranolaNativeError::OAuthStage {
            stage: "token_response",
            reason: "missing_refresh_path",
        })?;
    let scopes = token
        .scope
        .as_deref()
        .map(str::split_whitespace)
        .map(|scopes| scopes.map(str::to_string).collect())
        .unwrap_or_else(|| {
            REQUIRED_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect()
        });
    let expires_at = token.expires_at();
    Ok((
        client_id.to_string(),
        token.access_token.clone(),
        refresh,
        expires_at,
        scopes,
    ))
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    email: String,
    #[serde(default)]
    email_verified: Option<bool>,
}

fn verified_account(userinfo_endpoint: &str, access_token: &str) -> Result<String> {
    let response = discovery_http_client()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason: "client_unavailable",
        })?
        .get(userinfo_endpoint)
        .bearer_auth(access_token)
        .send()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason: "transport",
        })?;
    if !response.status().is_success() {
        let reason = classify_oauth_http_failure(response.status().as_u16(), None);
        return Err(GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason,
        }
        .into());
    }
    let identity: UserInfo = response
        .json()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason: "invalid_response",
        })?;
    if identity.email_verified == Some(false) {
        return Err(GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason: "identity_unverified",
        }
        .into());
    }
    normalize_granola_account(&identity.email).map_err(|_| {
        GranolaNativeError::OAuthStage {
            stage: "identity_verification",
            reason: "invalid_identity",
        }
        .into()
    })
}

fn ephemeral_loopback_redirect_uri() -> Result<String> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|_| GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "listener_unavailable",
        })?;
    let port = listener
        .local_addr()
        .map_err(|_| GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "listener_unavailable",
        })?
        .port();
    Ok(format!("http://127.0.0.1:{port}/oauth2/callback"))
}

fn validate_callback_url(
    callback: &str,
    expected_redirect_uri: &str,
    expected_state: &str,
) -> Result<String> {
    let expected =
        Url::parse(expected_redirect_uri).map_err(|_| GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "invalid_redirect",
        })?;
    let callback = Url::parse(callback.trim()).map_err(|_| GranolaNativeError::OAuthStage {
        stage: "browser_callback",
        reason: "invalid_callback",
    })?;
    if callback.scheme() != expected.scheme()
        || callback.host_str().map(str::to_ascii_lowercase)
            != expected.host_str().map(str::to_ascii_lowercase)
        || callback.port_or_known_default() != expected.port_or_known_default()
        || callback.path() != expected.path()
    {
        return Err(GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "redirect_mismatch",
        }
        .into());
    }
    let params = callback
        .query_pairs()
        .collect::<std::collections::BTreeMap<_, _>>();
    if params.get("state").map(|value| value.as_ref()) != Some(expected_state) {
        return Err(GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "state_mismatch",
        }
        .into());
    }
    if params.get("error").is_some() {
        return Err(GranolaNativeError::OAuthStage {
            stage: "browser_callback",
            reason: "authorization_denied",
        }
        .into());
    }
    params
        .get("code")
        .map(|value| value.to_string())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            GranolaNativeError::OAuthStage {
                stage: "browser_callback",
                reason: "missing_code",
            }
            .into()
        })
}

fn wait_for_callback(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String> {
    listener.set_nonblocking(true)?;
    let started = Instant::now();
    while started.elapsed() < timeout {
        match listener.accept() {
            Ok((mut stream, peer)) => {
                if !peer.ip().is_loopback() {
                    continue;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let mut buffer = [0u8; 8192];
                let count = stream.read(&mut buffer).unwrap_or(0);
                let request = String::from_utf8_lossy(&buffer[..count]);
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default();
                let _ = write_callback_response(&mut stream);
                let callback = Url::parse(&format!("http://127.0.0.1{path}")).map_err(|_| {
                    GranolaNativeError::OAuthStage {
                        stage: "browser_callback",
                        reason: "invalid_callback",
                    }
                })?;
                if callback.path() != "/oauth2/callback" {
                    continue;
                }
                let params = callback
                    .query_pairs()
                    .collect::<std::collections::BTreeMap<_, _>>();
                if params.get("state").map(|value| value.as_ref()) != Some(expected_state) {
                    return Err(GranolaNativeError::OAuthStage {
                        stage: "browser_callback",
                        reason: "state_mismatch",
                    }
                    .into());
                }
                if params.get("error").is_some() {
                    return Err(GranolaNativeError::OAuthStage {
                        stage: "browser_callback",
                        reason: "authorization_denied",
                    }
                    .into());
                }
                return params
                    .get("code")
                    .map(|value| value.to_string())
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| {
                        GranolaNativeError::OAuthStage {
                            stage: "browser_callback",
                            reason: "missing_code",
                        }
                        .into()
                    });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                return Err(GranolaNativeError::OAuthStage {
                    stage: "browser_callback",
                    reason: "listener_failed",
                }
                .into());
            }
        }
    }
    Err(GranolaNativeError::OAuthTimedOut.into())
}

fn write_callback_response(stream: &mut impl Write) -> std::io::Result<()> {
    let body = "<!doctype html><title>Granola connected</title><p>Return to Margins to finish connecting.</p>";
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn oauth_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("building Granola OAuth client")
}

fn discovery_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("building Granola HTTP client")
}

#[derive(Debug, Clone)]
pub struct GranolaMcpImportBatch {
    pub meetings: Vec<Meeting>,
    pub raw_payloads: Vec<Value>,
    pub warnings: Vec<String>,
    pub transcripts_plan_gated: bool,
}

pub fn fetch_granola_import_batch(
    margins_home: &Path,
    account: &str,
    backend: GranolaCredentialBackendKind,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<GranolaMcpImportBatch> {
    fetch_granola_import_batch_for_collection(
        margins_home,
        account,
        backend,
        &GranolaCollectionSelector::default_declaration(),
        progress,
    )
}

pub fn fetch_granola_import_batch_for_collection(
    margins_home: &Path,
    account: &str,
    backend: GranolaCredentialBackendKind,
    collection: &GranolaCollectionSelector,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<GranolaMcpImportBatch> {
    let account = normalize_granola_account(account)?;
    let store = GranolaAccountStore::new_with_backend(margins_home, &account, backend)?;
    let fetched = store.ensure_access_token().and_then(|access_token| {
        fetch_meetings(&access_token, GRANOLA_MCP_URL, collection, progress)
    });
    let (meetings, raw_payloads, warnings, transcripts_plan_gated) = match fetched {
        Ok(result) => result,
        Err(error) => {
            if granola_failure_info(&error)
                .is_some_and(|failure| failure.code == "granola_mcp_auth_failed")
            {
                let _ = store.forget();
            }
            return Err(error);
        }
    };
    Ok(GranolaMcpImportBatch {
        meetings,
        raw_payloads,
        warnings,
        transcripts_plan_gated,
    })
}

pub fn sync_granola_binding(
    margins_home: &Path,
    workspace_state_dir: &Path,
    account: &str,
    collection: &GranolaCollectionSelector,
    expected_workspace_revision: Option<&str>,
    backend: GranolaCredentialBackendKind,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<ReconcileResult> {
    let account = normalize_granola_account(account)?;
    let batch = fetch_granola_import_batch_for_collection(
        margins_home,
        &account,
        backend,
        collection,
        progress,
    )?;
    apply_granola_batch(
        workspace_state_dir,
        &account,
        collection,
        batch,
        expected_workspace_revision,
    )
}

fn apply_granola_batch(
    workspace_state_dir: &Path,
    account: &str,
    collection: &GranolaCollectionSelector,
    batch: GranolaMcpImportBatch,
    expected_workspace_revision: Option<&str>,
) -> Result<ReconcileResult> {
    let ctx = ConnectorCtx {
        vault_root: workspace_state_dir.to_path_buf(),
        connector_id: GRANOLA_CONNECTOR_ID.to_string(),
        account: account.to_string(),
        command_path: None,
    };
    let store = IntegrationsStore::open(workspace_state_dir)?;
    let result = (|| {
        let now = Utc::now();
        let mut documents = Vec::with_capacity(batch.meetings.len());
        let mut participants = Vec::new();
        let mut raw_items = Vec::new();
        for (index, meeting) in batch.meetings.iter().enumerate() {
            let source_id = stable_meeting_source_id(meeting)?;
            let occurred_at = meeting_occurred_at(meeting)?;
            documents.push(ExternalDocumentEvidence {
                source_id: source_id.clone(),
                occurred_at,
                title: meeting.title.clone(),
                body_text: meeting_body(meeting),
                href: meeting.provenance.clone(),
                attributes: serde_json::json!({
                    "granola_id": meeting.id,
                    "organizations": meeting.organizations,
                    "plan_gated_transcript": meeting.plan_gated_transcript,
                }),
            });
            for (position, person) in meeting.people.iter().enumerate() {
                participants.push(ExternalDocumentParticipant {
                    source_id: source_id.clone(),
                    participant_key: person
                        .email
                        .as_deref()
                        .map(str::trim)
                        .filter(|email| !email.is_empty())
                        .map(str::to_ascii_lowercase)
                        .unwrap_or_else(|| {
                            format!(
                                "position:{position}:{}",
                                person.name.trim().to_ascii_lowercase()
                            )
                        }),
                    position: u32::try_from(position).unwrap_or(u32::MAX),
                    display_name: person.name.clone(),
                    email: person.email.clone(),
                    ambiguous: !person.resolved,
                });
            }
            if let Some(payload) = batch.raw_payloads.get(index) {
                raw_items.push(RawItemDraft {
                    source_id,
                    payload: payload.clone(),
                });
            }
        }
        store.apply_external_document_delta(
            &ctx,
            ExternalDocumentDelta {
                documents,
                participants,
                tombstone_source_ids: Vec::new(),
                raw_items,
                snapshot_scope: Some(SurveyRange {
                    occurred_from: collection.occurred_from(now),
                    occurred_to: now,
                }),
                complete_snapshot: true,
                materialization_fingerprint: collection.materialization_fingerprint()?,
                next_cursor: None,
            },
            expected_workspace_revision,
        )
    })();
    if let Err(error) = &result {
        if error.downcast_ref::<WorkspaceMutationError>().is_none() {
            let _ = store.record_failed_reconcile(&ctx, &format!("{error:#}"));
        }
    }
    result
}

fn stable_meeting_source_id(meeting: &Meeting) -> Result<String> {
    meeting
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .with_context(|| "Granola meeting is missing a stable provider id")
}

fn meeting_occurred_at(meeting: &Meeting) -> Result<DateTime<Utc>> {
    meeting
        .created_at
        .as_deref()
        .and_then(crate::granola_import::parse_source_datetime)
        .with_context(|| "Granola meeting is missing a valid occurred_at timestamp")
}

fn meeting_body(meeting: &Meeting) -> String {
    let notes = meeting
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("_No notes were returned by Granola._");
    let transcript = meeting
        .transcript
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(if meeting.plan_gated_transcript {
            "_Transcript access was not available from the connected Granola account._"
        } else {
            "_No transcript was returned by Granola._"
        });
    format!("## Granola notes\n\n{notes}\n\n## Transcript\n\n{transcript}")
}

struct McpClient {
    http: Client,
    access_token: String,
    endpoint: String,
    session_id: Option<String>,
    next_id: i64,
}

#[derive(Debug, Clone)]
struct McpTool {
    name: String,
    input_schema: Value,
}

impl McpClient {
    fn new(access_token: &str, endpoint: &str) -> Result<Self> {
        let mut client = Self {
            http: Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|_| GranolaNativeError::Mcp {
                    code: "granola_mcp_client_unavailable",
                    stage: "mcp_initialize",
                    reason: "client_unavailable",
                })?,
            access_token: access_token.to_string(),
            endpoint: endpoint.to_string(),
            session_id: None,
            next_id: 1,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "margins", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        client.notify("notifications/initialized")?;
        Ok(client)
    }

    fn post(&mut self, body: &Value) -> Result<Response> {
        let mut request = self
            .http
            .post(&self.endpoint)
            .bearer_auth(&self.access_token)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION)
            .json(body);
        if let Some(session) = &self.session_id {
            request = request.header("Mcp-Session-Id", session);
        }
        let response = request.send().map_err(|_| GranolaNativeError::Mcp {
            code: "granola_mcp_transport_failed",
            stage: "mcp_call",
            reason: "transport",
        })?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }
        Ok(response)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let response =
            self.post(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let text = response.text().map_err(|_| GranolaNativeError::Mcp {
            code: "granola_mcp_transport_failed",
            stage: "mcp_call",
            reason: "transport",
        })?;
        if status.as_u16() == 401 {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_auth_failed",
                stage: "mcp_call",
                reason: "unauthorized",
            }
            .into());
        }
        if !status.is_success() {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_http_failed",
                stage: "mcp_call",
                reason: classify_oauth_http_failure(status.as_u16(), None),
            }
            .into());
        }
        let message = if content_type.contains("text/event-stream") {
            sse_response_for_id(&text, id).ok_or_else(|| GranolaNativeError::Mcp {
                code: "granola_mcp_protocol_failed",
                stage: "mcp_parse",
                reason: "missing_response",
            })?
        } else {
            serde_json::from_str(&text).map_err(|_| GranolaNativeError::Mcp {
                code: "granola_mcp_protocol_failed",
                stage: "mcp_parse",
                reason: "invalid_json",
            })?
        };
        if let Some(error) = message.get("error") {
            return Err(GranolaNativeError::Mcp {
                code: mcp_error_code(error),
                stage: "mcp_call",
                reason: "rpc_rejected",
            }
            .into());
        }
        Ok(message.get("result").cloned().unwrap_or(Value::Null))
    }

    fn notify(&mut self, method: &str) -> Result<()> {
        let response = self.post(&json!({"jsonrpc":"2.0","method":method}))?;
        if response.status().as_u16() == 401 {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_auth_failed",
                stage: "mcp_initialize",
                reason: "unauthorized",
            }
            .into());
        }
        if !response.status().is_success() {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_http_failed",
                stage: "mcp_initialize",
                reason: classify_oauth_http_failure(response.status().as_u16(), None),
            }
            .into());
        }
        Ok(())
    }

    fn list_tools(&mut self) -> Result<Vec<McpTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen = BTreeSet::new();
        let mut pages = 0usize;
        loop {
            if pages >= MAX_TOOLS_LIST_PAGES {
                return Err(GranolaNativeError::Mcp {
                    code: "granola_mcp_protocol_limit",
                    stage: "tools_list",
                    reason: "page_limit_exceeded",
                }
                .into());
            }
            pages += 1;
            let params = cursor
                .as_ref()
                .map(|cursor| json!({"cursor":cursor}))
                .unwrap_or_else(|| json!({}));
            let result = self.request("tools/list", params)?;
            let page = result
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| GranolaNativeError::Mcp {
                    code: "granola_mcp_protocol_failed",
                    stage: "tools_list",
                    reason: "invalid_response",
                })?;
            if tools.len().saturating_add(page.len()) > MAX_DISCOVERED_TOOLS {
                return Err(GranolaNativeError::Mcp {
                    code: "granola_mcp_protocol_limit",
                    stage: "tools_list",
                    reason: "tool_limit_exceeded",
                }
                .into());
            }
            for tool in page {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty())
                    .ok_or_else(|| GranolaNativeError::Mcp {
                        code: "granola_mcp_protocol_failed",
                        stage: "tools_list",
                        reason: "invalid_tool",
                    })?;
                tools.push(McpTool {
                    name: name.to_string(),
                    input_schema: tool
                        .get("inputSchema")
                        .or_else(|| tool.get("input_schema"))
                        .cloned()
                        .unwrap_or(Value::Null),
                });
            }
            let next = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_string);
            let Some(next) = next else {
                break;
            };
            if !seen.insert(next.clone()) {
                return Err(GranolaNativeError::Mcp {
                    code: "granola_mcp_pagination_failed",
                    stage: "tools_list",
                    reason: "repeated_cursor",
                }
                .into());
            }
            cursor = Some(next);
        }
        Ok(tools)
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let result = self.request("tools/call", json!({"name":name,"arguments":arguments}))?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            let payload = tool_payload(&result);
            return Err(GranolaNativeError::Mcp {
                code: mcp_error_code(&payload),
                stage: "tool_call",
                reason: "tool_rejected",
            }
            .into());
        }
        Ok(tool_payload(&result))
    }
}

fn mcp_error_code(error: &Value) -> &'static str {
    let text = match error {
        Value::String(value) => value.to_ascii_lowercase(),
        value => value.to_string().to_ascii_lowercase(),
    };
    if ["paid", "plan", "tier", "upgrade", "subscription"]
        .iter()
        .any(|marker| text.contains(marker))
    {
        "granola_transcript_plan_gated"
    } else {
        "granola_mcp_tool_failed"
    }
}

fn verify_mcp_access(access_token: &str, endpoint: &str, oauth_account: &str) -> Result<()> {
    let mut client = McpClient::new(access_token, endpoint)?;
    let tools = client.list_tools()?;
    if pick_tool(
        &tools,
        &[
            "list_meetings",
            "list_notes",
            "list_documents",
            "get_meetings",
        ],
    )
    .is_none()
    {
        return Err(GranolaNativeError::Mcp {
            code: "granola_mcp_capability_missing",
            stage: "tools_list",
            reason: "meeting_list_missing",
        }
        .into());
    }
    let account_tool =
        pick_tool(&tools, &["get_account_info"]).ok_or_else(|| GranolaNativeError::Mcp {
            code: "granola_mcp_account_capability_missing",
            stage: "account_verification",
            reason: "account_tool_missing",
        })?;
    let payload = match client.call_tool(&account_tool.name, json!({})) {
        Ok(payload) => payload,
        Err(error)
            if granola_failure_info(&error)
                .is_some_and(|failure| failure.code == "granola_mcp_tool_failed") =>
        {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_account_missing",
                stage: "account_verification",
                reason: "upstream_account_absent",
            }
            .into());
        }
        Err(error) => return Err(error),
    };
    let mcp_account = account_from_payload(&payload).ok_or_else(|| GranolaNativeError::Mcp {
        code: "granola_mcp_account_missing",
        stage: "account_verification",
        reason: "account_identity_missing",
    })?;
    if mcp_account != oauth_account {
        return Err(GranolaNativeError::AccountMismatch {
            expected: oauth_account.to_string(),
            actual: mcp_account,
        }
        .into());
    }
    Ok(())
}

fn fetch_meetings(
    access_token: &str,
    endpoint: &str,
    collection: &GranolaCollectionSelector,
    progress: &(dyn Fn(&str, usize, usize) + Send + Sync),
) -> Result<(Vec<Meeting>, Vec<Value>, Vec<String>, bool)> {
    progress("Connecting to Granola", 0, 0);
    let mut client = McpClient::new(access_token, endpoint)?;
    let tools = client.list_tools()?;
    let list_tool = pick_tool(
        &tools,
        &[
            "list_meetings",
            "list_notes",
            "list_documents",
            "get_meetings",
        ],
    )
    .ok_or_else(|| GranolaNativeError::Mcp {
        code: "granola_mcp_capability_missing",
        stage: "tools_list",
        reason: "meeting_list_missing",
    })?
    .clone();
    let detail_tool = pick_tool(
        &tools,
        &["get_meetings", "get_meeting", "get_notes", "get_note"],
    )
    .cloned();
    let transcript_tool = pick_tool(&tools, &["get_meeting_transcript", "get_transcript"]).cloned();
    validate_collection_capability(&list_tool, collection)?;
    progress("Listing meetings", 0, 0);
    let payload = client.call_tool(
        &list_tool.name,
        json!({
            "time_range": collection.time_range.as_provider_value(),
            "workspace_only": collection.workspace_only,
        }),
    )?;
    validate_complete_collection_payload(&payload)?;
    let pending = extract_meeting_values(&payload);
    validate_meetings_in_collection(&pending, collection, Utc::now())?;
    let total = pending.len();
    let mut meetings = Vec::with_capacity(total);
    let mut raw_payloads = Vec::with_capacity(total);
    let warnings = Vec::new();
    let mut plan_gated = false;
    for (index, value) in pending.iter().enumerate() {
        let mut meeting = crate::granola_import::meeting_from_value(value);
        let mut raw_payload = json!({"list": value});
        progress("Fetching Granola meeting details", index + 1, total);
        let id = meeting.id.clone();
        if let (Some(id), Some(tool)) = (&id, &detail_tool) {
            if meeting.notes.is_none() || meeting.people.is_empty() {
                match client.call_tool(&tool.name, json!({"meeting_ids":[id]})) {
                    Ok(payload) => {
                        raw_payload["detail"] = payload.clone();
                        if let Some(detail) = extract_meeting_values(&payload).first() {
                            let detail = crate::granola_import::meeting_from_value(detail);
                            meeting.notes = detail.notes.or(meeting.notes);
                            meeting.transcript = detail.transcript.or(meeting.transcript);
                            meeting.created_at = detail.created_at.or(meeting.created_at);
                            if !detail.people.is_empty() {
                                meeting.people = detail.people;
                                meeting.organizations = detail.organizations;
                            }
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        if meeting.transcript.is_none() {
            if let (Some(id), Some(tool)) = (&id, &transcript_tool) {
                match client.call_tool(&tool.name, json!({"meeting_id":id})) {
                    Ok(payload) => {
                        raw_payload["transcript"] = payload.clone();
                        meeting.transcript = transcript_from_payload(&payload);
                    }
                    Err(error) => {
                        let plan_gated_error = error.chain().any(|cause| {
                            cause
                                .downcast_ref::<GranolaNativeError>()
                                .is_some_and(|native| {
                                    native.code() == "granola_transcript_plan_gated"
                                })
                        });
                        if plan_gated_error {
                            meeting.plan_gated_transcript = true;
                            plan_gated = true;
                        } else {
                            return Err(error);
                        }
                    }
                }
            }
        }
        meetings.push(meeting);
        raw_payloads.push(raw_payload);
    }
    Ok((meetings, raw_payloads, warnings, plan_gated))
}

fn validate_collection_capability(
    list_tool: &McpTool,
    collection: &GranolaCollectionSelector,
) -> Result<()> {
    let properties = list_tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| GranolaNativeError::Mcp {
            code: "granola_mcp_collection_unsupported",
            stage: "collection_contract",
            reason: "input_schema_missing",
        })?;
    let supported_ranges = properties
        .get("time_range")
        .and_then(|value| value.get("enum"))
        .and_then(Value::as_array)
        .ok_or_else(|| GranolaNativeError::Mcp {
            code: "granola_mcp_collection_unsupported",
            stage: "collection_contract",
            reason: "time_range_unproven",
        })?;
    if !supported_ranges
        .iter()
        .any(|value| value.as_str() == Some(collection.time_range.as_provider_value()))
    {
        return Err(GranolaNativeError::Mcp {
            code: "granola_mcp_collection_unsupported",
            stage: "collection_contract",
            reason: "time_range_unsupported",
        }
        .into());
    }
    if properties
        .get("workspace_only")
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        != Some("boolean")
    {
        return Err(GranolaNativeError::Mcp {
            code: "granola_mcp_collection_unsupported",
            stage: "collection_contract",
            reason: "workspace_scope_unproven",
        }
        .into());
    }
    Ok(())
}

fn validate_complete_collection_payload(payload: &Value) -> Result<()> {
    fn flagged(value: &Value) -> bool {
        match value {
            Value::Object(values) => {
                values.iter().any(|(key, value)| {
                    (matches!(key.as_str(), "nextCursor" | "next_cursor")
                        && value
                            .as_str()
                            .is_some_and(|cursor| !cursor.trim().is_empty()))
                        || (matches!(key.as_str(), "limit" | "max_results" | "maxResults")
                            && value.as_u64().is_some_and(|limit| limit > 0))
                        || (matches!(key.as_str(), "hasMore" | "has_more" | "truncated")
                            && value.as_bool() == Some(true))
                }) || values.values().any(flagged)
            }
            Value::Array(values) => values.iter().any(flagged),
            Value::String(text) => {
                let lowercase = text.to_ascii_lowercase();
                lowercase.contains("truncated=\"true\"")
                    || lowercase.contains("has_more=\"true\"")
                    || lowercase.contains("hasmore=\"true\"")
                    || lowercase.contains("limit=\"")
                    || lowercase.contains("<nextcursor>")
                    || lowercase.contains("<next_cursor>")
            }
            _ => false,
        }
    }
    if flagged(payload) {
        return Err(GranolaNativeError::Mcp {
            code: "granola_mcp_collection_incomplete",
            stage: "meeting_enumeration",
            reason: "upstream_truncated",
        }
        .into());
    }
    let meetings = extract_meeting_values(payload);
    let declared_empty = match payload {
        Value::Array(values) => values.is_empty(),
        Value::Object(values) => [
            "meetings",
            "notes",
            "documents",
            "docs",
            "items",
            "results",
            "data",
        ]
        .iter()
        .any(|key| {
            values
                .get(*key)
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        }),
        Value::String(text) => {
            let compact = text
                .split_whitespace()
                .collect::<String>()
                .to_ascii_lowercase();
            compact.contains("count=\"0\"")
                || compact == "<meetings_data></meetings_data>"
                || compact == "<meetings></meetings>"
        }
        _ => false,
    };
    if meetings.is_empty() && !declared_empty {
        return Err(GranolaNativeError::Mcp {
            code: "granola_mcp_parse_failed",
            stage: "meeting_parse",
            reason: "unrecognized_collection",
        }
        .into());
    }
    Ok(())
}

fn validate_meetings_in_collection(
    values: &[Value],
    collection: &GranolaCollectionSelector,
    now: DateTime<Utc>,
) -> Result<()> {
    let occurred_from = collection.occurred_from(now);
    for value in values {
        let meeting = crate::granola_import::meeting_from_value(value);
        if meeting.id.as_deref().is_none_or(|id| id.trim().is_empty()) {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_parse_failed",
                stage: "meeting_parse",
                reason: "meeting_id_missing",
            }
            .into());
        }
        let occurred_at = meeting
            .created_at
            .as_deref()
            .and_then(crate::granola_import::parse_source_datetime)
            .ok_or_else(|| GranolaNativeError::Mcp {
                code: "granola_mcp_parse_failed",
                stage: "meeting_parse",
                reason: "meeting_time_invalid",
            })?;
        if occurred_at < occurred_from || occurred_at > now {
            return Err(GranolaNativeError::Mcp {
                code: "granola_mcp_collection_incomplete",
                stage: "meeting_enumeration",
                reason: "meeting_outside_declared_window",
            }
            .into());
        }
    }
    Ok(())
}

fn pick_tool<'a>(tools: &'a [McpTool], wanted: &[&str]) -> Option<&'a McpTool> {
    wanted.iter().find_map(|wanted| {
        tools
            .iter()
            .find(|tool| tool.name.eq_ignore_ascii_case(wanted))
    })
}

fn account_from_payload(payload: &Value) -> Option<String> {
    fn from_text(text: &str) -> Option<String> {
        for tag in ["email", "account_email", "user_email"] {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            if let Some(candidate) = text
                .split_once(&open)
                .and_then(|(_, rest)| rest.split_once(&close).map(|(value, _)| value))
                .and_then(|value| normalize_granola_account(value).ok())
            {
                return Some(candidate);
            }
        }
        None
    }

    match payload {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .as_ref()
            .and_then(account_from_payload)
            .or_else(|| from_text(text)),
        Value::Array(values) => values.iter().find_map(account_from_payload),
        Value::Object(values) => {
            for key in ["email", "account_email", "accountEmail", "user_email"] {
                if let Some(account) = values
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(|value| normalize_granola_account(value).ok())
                {
                    return Some(account);
                }
            }
            ["account", "user", "profile"]
                .iter()
                .find_map(|key| values.get(*key).and_then(account_from_payload))
        }
        _ => None,
    }
}

fn tool_payload(result: &Value) -> Value {
    if let Some(value) = result.get("structuredContent") {
        return value.clone();
    }
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

fn sse_response_for_id(text: &str, id: i64) -> Option<Value> {
    let mut data = String::new();
    let check = |data: &mut String| -> Option<Value> {
        if data.is_empty() {
            return None;
        }
        let parsed = serde_json::from_str::<Value>(data).ok();
        data.clear();
        parsed.filter(|value| value.get("id").and_then(Value::as_i64) == Some(id))
    };
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim_start());
        } else if line.trim().is_empty() {
            if let Some(found) = check(&mut data) {
                return Some(found);
            }
        }
    }
    check(&mut data)
}

fn extract_meeting_values(payload: &Value) -> Vec<Value> {
    if let Value::String(text) = payload {
        if text.trim_start().starts_with('<') {
            return parse_xml_meetings(text);
        }
    }
    if let Some(values) = payload.as_array() {
        return values.clone();
    }
    for key in [
        "meetings",
        "notes",
        "documents",
        "docs",
        "items",
        "results",
        "data",
    ] {
        if let Some(values) = payload.get(key).and_then(Value::as_array) {
            return values.clone();
        }
    }
    if payload.is_object() && payload.get("id").is_some() {
        return vec![payload.clone()];
    }
    Vec::new()
}

fn transcript_from_payload(payload: &Value) -> Option<String> {
    match payload {
        Value::Array(items) => crate::granola_import::transcript_turns(items),
        Value::String(text) => (!text.trim().is_empty()).then(|| text.trim().to_string()),
        Value::Object(_) => [
            "transcript",
            "segments",
            "utterances",
            "entries",
            "transcript_segments",
        ]
        .iter()
        .find_map(|key| {
            payload
                .get(*key)
                .and_then(Value::as_array)
                .and_then(|items| crate::granola_import::transcript_turns(items))
                .or_else(|| {
                    payload
                        .get(*key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string)
                })
        }),
        _ => None,
    }
}

fn parse_xml_meetings(xml: &str) -> Vec<Value> {
    fn decode(value: &str) -> String {
        value
            .replace("&amp;", "&")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
    }
    fn attr(tag: &str, name: &str) -> Option<String> {
        let marker = format!("{name}=\"");
        let value = tag.split_once(&marker)?.1;
        Some(decode(value.split_once('"')?.0))
    }
    fn child(body: &str, name: &str) -> Option<String> {
        let open = format!("<{name}>");
        let close = format!("</{name}>");
        let value = body.split_once(&open)?.1.split_once(&close)?.0;
        Some(decode(value.trim()))
    }
    fn participants(value: &str) -> Value {
        Value::Array(
            value
                .split(',')
                .filter_map(|entry| {
                    let entry = entry.trim();
                    if entry.is_empty() {
                        return None;
                    }
                    let (raw_name, email) = match (entry.rfind('<'), entry.rfind('>')) {
                        (Some(left), Some(right)) if left < right => {
                            (&entry[..left], Some(entry[left + 1..right].trim()))
                        }
                        _ => (entry, None),
                    };
                    let name = raw_name
                        .split_once('(')
                        .map(|(name, _)| name)
                        .unwrap_or(raw_name)
                        .trim();
                    (!name.is_empty()).then(|| json!({"name":name,"email":email}))
                })
                .collect(),
        )
    }
    let mut remaining = xml;
    let mut values = Vec::new();
    while let Some(start) = remaining.find("<meeting ") {
        remaining = &remaining[start + 1..];
        let Some(tag_end) = remaining.find('>') else {
            break;
        };
        let tag = &remaining[..tag_end];
        remaining = &remaining[tag_end + 1..];
        let body_end = remaining.find("</meeting>").unwrap_or(0);
        let body = &remaining[..body_end];
        if body_end > 0 {
            remaining = &remaining[body_end + "</meeting>".len()..];
        }
        let mut value = serde_json::Map::new();
        for (attribute, key) in [("id", "id"), ("title", "title"), ("date", "date")] {
            if let Some(found) = attr(tag, attribute) {
                value.insert(key.to_string(), Value::String(found));
            }
        }
        if let Some(found) = child(body, "known_participants") {
            value.insert("participants".into(), participants(&found));
        }
        if let Some(found) = child(body, "summary") {
            if !matches!(
                found.trim().to_ascii_lowercase().as_str(),
                "" | "no summary" | "no summary available" | "no notes" | "n/a"
            ) {
                value.insert("notes".into(), Value::String(found));
            }
        }
        values.push(Value::Object(value));
    }
    values
}

#[cfg(unix)]
fn validate_private_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(GranolaNativeError::CredentialsUnavailable(
            "private credential path is not a directory".into(),
        )
        .into());
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(GranolaNativeError::StoragePermission {
            path: path.to_path_buf(),
            mode,
        }
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn validate_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(GranolaNativeError::CredentialsUnavailable(
            "private credential path is not a regular file".into(),
        )
        .into());
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(GranolaNativeError::StoragePermission {
            path: path.to_path_buf(),
            mode,
        }
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<()> {
    write_private_bytes(path, &serde_json::to_vec_pretty(value)?)
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("private file path has no parent")?;
    ensure_private_dir_chain(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.persist(path).map_err(|error| error.error)?;
    validate_private_file_permissions(path)
}

fn ensure_private_dir_chain(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(GranolaNativeError::CredentialsUnavailable(
                "private credential parent is not a directory".into(),
            )
            .into());
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::HealthStatus;

    struct PanicPresenter;

    impl GranolaOAuthPresenter for PanicPresenter {
        fn present_authorization_url(
            &self,
            _url: &str,
            _need_callback: bool,
        ) -> std::result::Result<String, String> {
            panic!("backend ownership preflight must run before OAuth presentation")
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return bytes;
            }
        }
        panic!("mock MCP request ended before its JSON body")
    }

    fn read_http_json(stream: &mut std::net::TcpStream) -> Value {
        let bytes = read_http_request(stream);
        let header_end = bytes
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap();
        serde_json::from_slice(&bytes[header_end + 4..]).unwrap()
    }

    fn write_http_json(stream: &mut std::net::TcpStream, status: &str, body: Option<Value>) {
        let encoded = body
            .map(|value| serde_json::to_vec(&value).unwrap())
            .unwrap_or_default();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            encoded.len()
        )
        .unwrap();
        stream.write_all(&encoded).unwrap();
    }

    fn write_http_text(stream: &mut std::net::TcpStream, status: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body.as_bytes()).unwrap();
    }

    fn committed_file_store(home: &Path, expires_at: Option<DateTime<Utc>>) -> GranolaAccountStore {
        let store = GranolaAccountStore::new_with_backend(
            home,
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
        )
        .unwrap();
        store
            .commit(
                "fixture-client",
                &REQUIRED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>(),
                StoredToken {
                    schema_version: STORAGE_SCHEMA.into(),
                    account: "owner@example.com".into(),
                    client_id: "fixture-client".into(),
                    access_token: "fixture-access".into(),
                    refresh_token: "fixture-refresh".into(),
                    expires_at,
                },
            )
            .unwrap();
        store
    }

    #[test]
    fn orphaned_token_without_metadata_is_not_connected() {
        let temp = tempfile::tempdir().unwrap();
        let store = GranolaAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
        )
        .unwrap();
        ensure_private_dir_chain(store.account_dir()).unwrap();
        let token = StoredToken {
            schema_version: STORAGE_SCHEMA.into(),
            account: "owner@example.com".into(),
            client_id: "client".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: None,
        };
        store.store_token(&token).unwrap();
        let error = store.durable_connection_metadata().unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<GranolaNativeError>()
                .map(GranolaNativeError::code),
            Some("granola_credentials_unavailable")
        );
    }

    #[test]
    fn ownership_boundary_rejects_other_owner_before_oauth_token_access_or_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = GranolaAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::OsKeyring,
        )
        .unwrap();
        ensure_private_dir_chain(desktop.account_dir().parent().unwrap()).unwrap();
        ensure_private_dir_chain(desktop.account_dir()).unwrap();
        write_private_json(
            &desktop.metadata_path(),
            &GranolaConnectionMetadata {
                schema_version: METADATA_SCHEMA.into(),
                account: "owner@example.com".into(),
                storage: GranolaCredentialBackendKind::OsKeyring,
                client_id: "desktop-client".into(),
                scopes: REQUIRED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect(),
                connected_at: Utc::now(),
            },
        )
        .unwrap();
        let metadata_before = std::fs::read(desktop.metadata_path()).unwrap();

        let cli = GranolaAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
        )
        .unwrap();
        let pre_oauth = connect_granola_account(
            temp.path(),
            Some("owner@example.com"),
            GranolaCredentialBackendKind::File0600,
            GranolaOAuthMode::BrowserLoopback,
            &PanicPresenter,
        )
        .unwrap_err();
        assert_eq!(
            pre_oauth
                .downcast_ref::<GranolaNativeError>()
                .map(GranolaNativeError::code),
            Some("granola_credential_backend_mismatch")
        );
        for error in [
            cli.durable_connection_metadata().unwrap_err(),
            cli.forget().unwrap_err(),
        ] {
            assert_eq!(
                error
                    .downcast_ref::<GranolaNativeError>()
                    .map(GranolaNativeError::code),
                Some("granola_credential_backend_mismatch")
            );
            assert!(!format!("{error:#}").contains("owner@example.com"));
        }
        assert_eq!(
            std::fs::read(desktop.metadata_path()).unwrap(),
            metadata_before
        );
        assert!(!cli.token_path().exists());

        let known_identity = commit_verified_connection(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
            "cli-client",
            &REQUIRED_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<Vec<_>>(),
            StoredToken {
                schema_version: STORAGE_SCHEMA.into(),
                account: "owner@example.com".into(),
                client_id: "cli-client".into(),
                access_token: "new-access".into(),
                refresh_token: "new-refresh".into(),
                expires_at: None,
            },
            "http://127.0.0.1:1/mcp",
        )
        .unwrap_err();
        assert_eq!(
            known_identity
                .downcast_ref::<GranolaNativeError>()
                .map(GranolaNativeError::code),
            Some("granola_credential_backend_mismatch")
        );

        let error = cli
            .commit(
                "cli-client",
                &REQUIRED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>(),
                StoredToken {
                    schema_version: STORAGE_SCHEMA.into(),
                    account: "owner@example.com".into(),
                    client_id: "cli-client".into(),
                    access_token: "new-access".into(),
                    refresh_token: "new-refresh".into(),
                    expires_at: None,
                },
            )
            .unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<GranolaNativeError>()
                .map(GranolaNativeError::code),
            Some("granola_credential_backend_mismatch")
        );
        assert_eq!(
            std::fs::read(desktop.metadata_path()).unwrap(),
            metadata_before
        );
        assert!(!cli.token_path().exists());
    }

    #[test]
    fn ownership_boundary_propagates_prior_token_snapshot_errors_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let store = committed_file_store(temp.path(), None);
        write_private_bytes(&store.token_path(), b"not-json").unwrap();
        let metadata_before = std::fs::read(store.metadata_path()).unwrap();
        let token_before = std::fs::read(store.token_path()).unwrap();

        let error = store
            .commit(
                "new-client",
                &REQUIRED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>(),
                StoredToken {
                    schema_version: STORAGE_SCHEMA.into(),
                    account: "owner@example.com".into(),
                    client_id: "new-client".into(),
                    access_token: "new-access".into(),
                    refresh_token: "new-refresh".into(),
                    expires_at: None,
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("parsing Granola token cache"));
        assert_eq!(
            std::fs::read(store.metadata_path()).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(store.token_path()).unwrap(), token_before);
    }

    #[test]
    fn file_connection_requires_private_permissions_and_coherent_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = GranolaAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
        )
        .unwrap();
        let token = StoredToken {
            schema_version: STORAGE_SCHEMA.into(),
            account: "owner@example.com".into(),
            client_id: "client".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: None,
        };
        store
            .commit(
                "client",
                &REQUIRED_SCOPES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<Vec<_>>(),
                token,
            )
            .unwrap();
        assert_eq!(
            store.durable_connection_metadata().unwrap().account,
            "owner@example.com"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(store.account_dir().parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(store.account_dir())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(store.token_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(store.metadata_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            std::fs::set_permissions(store.account_dir(), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            assert_eq!(
                store
                    .durable_connection_metadata()
                    .unwrap_err()
                    .downcast_ref::<GranolaNativeError>()
                    .map(GranolaNativeError::code),
                Some("granola_storage_permission_insecure")
            );
        }
    }

    #[test]
    fn durable_status_rejects_missing_scope_even_with_a_refresh_path() {
        let temp = tempfile::tempdir().unwrap();
        let store = GranolaAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GranolaCredentialBackendKind::File0600,
        )
        .unwrap();
        let token = StoredToken {
            schema_version: STORAGE_SCHEMA.into(),
            account: "owner@example.com".into(),
            client_id: "client".into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: None,
        };
        let incomplete = vec!["openid".to_string(), "profile".to_string()];
        let error = store.commit("client", &incomplete, token).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<GranolaNativeError>()
                .map(GranolaNativeError::code),
            Some("granola_credentials_unavailable")
        );
        assert!(!store.account_dir().exists() || store.metadata().unwrap().is_none());
    }

    #[test]
    fn expired_access_refreshes_and_atomically_persists_a_usable_token() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = String::from_utf8(read_http_request(&mut stream)).unwrap();
            assert!(request.contains("grant_type=refresh_token"));
            write_http_json(
                &mut stream,
                "200 OK",
                Some(json!({
                    "access_token": "refreshed-access",
                    "expires_in": 3600
                })),
            );
        });
        let home = tempfile::tempdir().unwrap();
        let store =
            committed_file_store(home.path(), Some(Utc::now() - chrono::Duration::minutes(5)));
        let metadata = store.durable_connection_metadata().unwrap();
        let access = store
            .ensure_access_token_for_metadata(&metadata, || Ok(endpoint))
            .unwrap();
        server.join().unwrap();
        assert_eq!(access, "refreshed-access");
        let persisted = store.load_token().unwrap().unwrap();
        assert_eq!(persisted.access_token, "refreshed-access");
        assert_eq!(persisted.refresh_token, "fixture-refresh");
        assert!(persisted.access_is_usable());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(store.token_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn expired_access_invalid_grant_invalidates_machine_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_http_request(&mut stream);
            write_http_json(
                &mut stream,
                "400 Bad Request",
                Some(json!({"error":"invalid_grant"})),
            );
        });
        let home = tempfile::tempdir().unwrap();
        let store =
            committed_file_store(home.path(), Some(Utc::now() - chrono::Duration::minutes(5)));
        let metadata = store.durable_connection_metadata().unwrap();
        let error = store
            .ensure_access_token_for_metadata(&metadata, || Ok(endpoint))
            .unwrap_err();
        server.join().unwrap();
        let failure = granola_failure_info(&error).unwrap();
        assert_eq!(failure.stage, "refresh");
        assert_eq!(failure.reason, "invalid_grant");
        assert!(!failure.retryable);
        assert!(!store.account_dir().exists());
    }

    #[test]
    fn expired_access_transport_failure_is_retryable_and_preserves_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
        drop(listener);
        let home = tempfile::tempdir().unwrap();
        let store =
            committed_file_store(home.path(), Some(Utc::now() - chrono::Duration::minutes(5)));
        let before = std::fs::read(store.token_path()).unwrap();
        let metadata = store.durable_connection_metadata().unwrap();
        let error = store
            .ensure_access_token_for_metadata(&metadata, || Ok(endpoint))
            .unwrap_err();
        let failure = granola_failure_info(&error).unwrap();
        assert_eq!(failure.stage, "refresh");
        assert_eq!(failure.reason, "transport");
        assert!(failure.retryable);
        assert_eq!(std::fs::read(store.token_path()).unwrap(), before);
    }

    #[test]
    fn unexpired_access_never_resolves_or_calls_a_refresh_endpoint() {
        let home = tempfile::tempdir().unwrap();
        let store =
            committed_file_store(home.path(), Some(Utc::now() + chrono::Duration::hours(1)));
        let metadata = store.durable_connection_metadata().unwrap();
        let access = store
            .ensure_access_token_for_metadata(&metadata, || {
                panic!("unexpired status attempted network discovery")
            })
            .unwrap();
        assert_eq!(access, "fixture-access");
    }

    #[test]
    fn live_xml_shape_materializes_provider_ids_and_participants() {
        let xml = r#"<meetings_data count="1"><meeting id="record-1" title="Review" date="Aug 26, 2026"><known_participants>Owner (note creator) &lt;owner@example.com&gt;, Guest &lt;guest@example.com&gt;</known_participants><summary>Decision</summary></meeting></meetings_data>"#;
        let values = parse_xml_meetings(xml);
        assert_eq!(values.len(), 1);
        let meeting = crate::granola_import::meeting_from_value(&values[0]);
        assert_eq!(meeting.id.as_deref(), Some("record-1"));
        assert_eq!(meeting.people.len(), 2);
        assert_eq!(meeting.notes.as_deref(), Some("Decision"));
    }

    #[test]
    fn streamable_http_mcp_fetches_only_supported_record_shapes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let meeting_date = Utc::now().format("%b %e, %Y").to_string();
        let server = std::thread::spawn(move || {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_json(&mut stream);
                let method = request["method"].as_str().unwrap();
                match method {
                    "initialize" => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}}),
                        ),
                    ),
                    "notifications/initialized" => {
                        write_http_json(&mut stream, "202 Accepted", None)
                    }
                    "tools/list" => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[{"name":"list_meetings","inputSchema":{"type":"object","properties":{"time_range":{"type":"string","enum":["last_30_days"]},"workspace_only":{"type":"boolean"}}}}]}}),
                        ),
                    ),
                    "tools/call" => {
                        assert_eq!(request["params"]["name"], "list_meetings");
                        assert_eq!(request["params"]["arguments"]["time_range"], "last_30_days");
                        assert_eq!(request["params"]["arguments"]["workspace_only"], false);
                        write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(
                                json!({"jsonrpc":"2.0","id":request["id"],"result":{"content":[{"type":"text","text":format!("<meetings_data count=\"1\"><meeting id=\"remote-1\" title=\"Review\" date=\"{meeting_date}\"><known_participants>Guest &lt;guest@example.com&gt;</known_participants><summary>Decision</summary></meeting></meetings_data>")}]}}),
                            ),
                        )
                    }
                    other => panic!("unexpected MCP method: {other}"),
                }
            }
        });
        let (meetings, raw_payloads, warnings, plan_gated) = fetch_meetings(
            "fixture-access",
            &endpoint,
            &GranolaCollectionSelector::default_declaration(),
            &|_, _, _| {},
        )
        .unwrap();
        server.join().unwrap();
        assert!(warnings.is_empty());
        assert!(!plan_gated);
        assert_eq!(meetings.len(), 1);
        assert_eq!(raw_payloads.len(), 1);
        assert_eq!(meetings[0].id.as_deref(), Some("remote-1"));
        assert_eq!(meetings[0].people.len(), 1);
    }

    #[test]
    fn granola_batch_materializes_under_declared_account_and_selector() {
        let temp = tempfile::tempdir().unwrap();
        let collection = GranolaCollectionSelector::default_declaration();
        let occurred_at = (Utc::now() - chrono::Duration::days(1)).to_rfc3339();
        let result = apply_granola_batch(
            temp.path(),
            "owner@example.com",
            &collection,
            GranolaMcpImportBatch {
                meetings: vec![Meeting {
                    id: Some("granola-remote-1".to_string()),
                    title: "Migration review".to_string(),
                    created_at: Some(occurred_at),
                    notes: Some("Keep online source binding.".to_string()),
                    transcript: Some("Ada: Sync from Granola.".to_string()),
                    people: vec![crate::granola_import::Person {
                        name: "Ada Lovelace".to_string(),
                        email: Some("Ada@Example.COM".to_string()),
                        organizations: vec!["Example".to_string()],
                        resolved: true,
                    }],
                    organizations: vec!["Example".to_string()],
                    provenance: Some("https://granola.ai/meetings/granola-remote-1".to_string()),
                    plan_gated_transcript: false,
                }],
                raw_payloads: vec![json!({"id":"granola-remote-1","fixture":true})],
                warnings: Vec::new(),
                transcripts_plan_gated: false,
            },
            None,
        )
        .unwrap();

        assert_eq!(result.records_written, 1);
        let store = IntegrationsStore::open(temp.path()).unwrap();
        let ctx = ConnectorCtx {
            vault_root: temp.path().to_path_buf(),
            connector_id: GRANOLA_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let documents = store.external_document_evidence(&ctx).unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].source_id, "granola-remote-1");
        assert!(documents[0].body_text.contains("## Granola notes"));
        assert!(documents[0].body_text.contains("## Transcript"));
        assert_eq!(
            store.external_document_participants(&ctx).unwrap()[0].participant_key,
            "ada@example.com"
        );
        let report = store
            .health_report_for_materialization(
                &ctx,
                &collection.materialization_fingerprint().unwrap(),
            )
            .unwrap();
        assert_eq!(report.status, HealthStatus::Fresh);

        let other_account = ConnectorCtx {
            account: "other@example.com".to_string(),
            ..ctx
        };
        assert!(store
            .external_document_evidence(&other_account)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn bounded_meeting_enumeration_rejects_truncation_and_out_of_window_rows() {
        let outside_date = (Utc::now() - chrono::Duration::days(45))
            .format("%b %e, %Y")
            .to_string();
        for (payload, expected_reason) in [
            (
                json!({
                    "meetings": [],
                    "truncated": true,
                    "provider_text": "SENTINEL https://sentinel.invalid/private"
                }),
                "upstream_truncated",
            ),
            (
                Value::String(format!(
                    "<meetings_data count=\"1\"><meeting id=\"old-1\" title=\"Old\" date=\"{outside_date}\"><summary>old</summary></meeting></meetings_data>"
                )),
                "meeting_outside_declared_window",
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                for call in 0..4 {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_json(&mut stream);
                    match call {
                        0 => write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}})),
                        ),
                        1 => write_http_json(&mut stream, "202 Accepted", None),
                        2 => write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[{"name":"list_meetings","inputSchema":{"type":"object","properties":{"time_range":{"type":"string","enum":["last_30_days"]},"workspace_only":{"type":"boolean"}}}}]}})),
                        ),
                        3 => write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(json!({"jsonrpc":"2.0","id":request["id"],"result":{"structuredContent":payload}})),
                        ),
                        _ => unreachable!(),
                    }
                }
            });
            let error = fetch_meetings(
                "fixture-access",
                &endpoint,
                &GranolaCollectionSelector::default_declaration(),
                &|_, _, _| {},
            )
            .unwrap_err();
            server.join().unwrap();
            let failure = granola_failure_info(&error).unwrap();
            assert_eq!(failure.reason, expected_reason);
            assert!(!failure.retryable);
            let rendered = format!("{error:#}");
            assert!(!rendered.contains("sentinel.invalid"));
            assert!(!rendered.contains("SENTINEL"));
            assert!(!rendered.contains(&endpoint));
        }
    }

    #[test]
    fn verified_mcp_account_is_transactional_and_reads_later_tool_pages() {
        enum AccountReply {
            Payload(Value),
            Absent,
        }
        for (reply, expected_code) in [
            (
                AccountReply::Payload(json!({"email":"different@example.com"})),
                "granola_account_mismatch",
            ),
            (
                AccountReply::Payload(json!({"active_workspace":"fixture"})),
                "granola_mcp_account_missing",
            ),
            (
                AccountReply::Payload(json!({
                    "participants":[{"email":"owner@example.com"}]
                })),
                "granola_mcp_account_missing",
            ),
            (AccountReply::Absent, "granola_mcp_account_missing"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                for call in 0..5 {
                    let (mut stream, _) = listener.accept().unwrap();
                    let request = read_http_json(&mut stream);
                    match call {
                        0 => write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(
                                json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}}),
                            ),
                        ),
                        1 => write_http_json(&mut stream, "202 Accepted", None),
                        2 => {
                            assert_eq!(request["method"], "tools/list");
                            assert!(request["params"].get("cursor").is_none());
                            write_http_json(
                                &mut stream,
                                "200 OK",
                                Some(
                                    json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[{"name":"list_meetings"}],"nextCursor":"later-tools"}}),
                                ),
                            );
                        }
                        3 => {
                            assert_eq!(request["params"]["cursor"], "later-tools");
                            write_http_json(
                                &mut stream,
                                "200 OK",
                                Some(
                                    json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[{"name":"get_account_info"}]}}),
                                ),
                            );
                        }
                        4 => {
                            assert_eq!(request["method"], "tools/call");
                            let result = match &reply {
                                AccountReply::Payload(payload) => {
                                    json!({"structuredContent":payload})
                                }
                                AccountReply::Absent => json!({
                                    "isError":true,
                                    "content":[{"type":"text","text":"SENTINEL provider account absent https://sentinel.invalid/private"}]
                                }),
                            };
                            write_http_json(
                                &mut stream,
                                "200 OK",
                                Some(json!({"jsonrpc":"2.0","id":request["id"],"result":result})),
                            );
                        }
                        _ => unreachable!(),
                    }
                }
            });
            let home = tempfile::tempdir().unwrap();
            let token = StoredToken {
                schema_version: STORAGE_SCHEMA.into(),
                account: "owner@example.com".into(),
                client_id: "fixture-client".into(),
                access_token: "fixture-access".into(),
                refresh_token: "fixture-refresh".into(),
                expires_at: None,
            };
            let error = commit_verified_connection(
                home.path(),
                "owner@example.com",
                GranolaCredentialBackendKind::File0600,
                "fixture-client",
                &REQUIRED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect::<Vec<_>>(),
                token,
                &endpoint,
            )
            .unwrap_err();
            server.join().unwrap();
            assert_eq!(granola_failure_info(&error).unwrap().code, expected_code);
            assert!(!format!("{error:#}").contains("sentinel.invalid"));
            assert!(!home.path().join("granola").exists());
        }
    }

    #[test]
    fn tools_list_repeated_cursor_fails_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for call in 0..4 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_json(&mut stream);
                match call {
                    0 => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}}),
                        ),
                    ),
                    1 => write_http_json(&mut stream, "202 Accepted", None),
                    _ => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[],"nextCursor":"same-cursor"}}),
                        ),
                    ),
                }
            }
        });
        let mut client = McpClient::new("fixture-access", &endpoint).unwrap();
        let error = client.list_tools().unwrap_err();
        server.join().unwrap();
        let failure = granola_failure_info(&error).unwrap();
        assert_eq!(failure.code, "granola_mcp_pagination_failed");
        assert_eq!(failure.reason, "repeated_cursor");
        assert!(!failure.retryable);
    }

    #[test]
    fn tools_list_endless_unique_cursors_hits_a_bounded_protocol_limit() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for call in 0..(MAX_TOOLS_LIST_PAGES + 2) {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_json(&mut stream);
                match call {
                    0 => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}
                        })),
                    ),
                    1 => write_http_json(&mut stream, "202 Accepted", None),
                    page => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{
                                "tools":[],
                                "nextCursor":format!("unique-cursor-{page}")
                            }
                        })),
                    ),
                }
            }
        });
        let mut client = McpClient::new("fixture-access", &endpoint).unwrap();
        let error = client.list_tools().unwrap_err();
        server.join().unwrap();
        let failure = granola_failure_info(&error).unwrap();
        assert_eq!(failure.code, "granola_mcp_protocol_limit");
        assert_eq!(failure.stage, "tools_list");
        assert_eq!(failure.reason, "page_limit_exceeded");
        assert!(!failure.retryable);
    }

    #[test]
    fn tools_list_over_tool_count_rejects_the_entire_capability_set() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for call in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_json(&mut stream);
                match call {
                    0 => write_http_json(
                        &mut stream,
                        "200 OK",
                        Some(json!({
                            "jsonrpc":"2.0",
                            "id":request["id"],
                            "result":{"protocolVersion":PROTOCOL_VERSION,"capabilities":{}}
                        })),
                    ),
                    1 => write_http_json(&mut stream, "202 Accepted", None),
                    2 => {
                        let tools = (0..=MAX_DISCOVERED_TOOLS)
                            .map(|index| json!({"name":format!("tool-{index}")}))
                            .collect::<Vec<_>>();
                        write_http_json(
                            &mut stream,
                            "200 OK",
                            Some(json!({
                                "jsonrpc":"2.0",
                                "id":request["id"],
                                "result":{"tools":tools}
                            })),
                        );
                    }
                    _ => unreachable!(),
                }
            }
        });
        let mut client = McpClient::new("fixture-access", &endpoint).unwrap();
        let error = client.list_tools().unwrap_err();
        server.join().unwrap();
        let failure = granola_failure_info(&error).unwrap();
        assert_eq!(failure.code, "granola_mcp_protocol_limit");
        assert_eq!(failure.stage, "tools_list");
        assert_eq!(failure.reason, "tool_limit_exceeded");
        assert!(!failure.retryable);
    }

    #[test]
    fn oauth_http_failures_are_redacted_and_retry_only_when_transient() {
        for (status, standard_error, expected_reason, retryable) in [
            ("400 Bad Request", "invalid_grant", "invalid_grant", false),
            ("429 Too Many Requests", "slow_down", "rate_limited", true),
            (
                "503 Service Unavailable",
                "temporarily_unavailable",
                "server_unavailable",
                true,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let _request = read_http_request(&mut stream);
                write_http_json(
                    &mut stream,
                    status,
                    Some(json!({
                        "error":standard_error,
                        "error_description":"SENTINEL provider text https://sentinel.invalid/private?code=secret"
                    })),
                );
            });
            let client = oauth_http_client().unwrap();
            let error = exchange_token(
                &client,
                &endpoint,
                "browser_token_exchange",
                &[("grant_type", "authorization_code")],
            )
            .unwrap_err();
            server.join().unwrap();
            let failure = granola_failure_info(&error).unwrap();
            assert_eq!(failure.reason, expected_reason);
            assert_eq!(failure.retryable, retryable);
            let rendered = format!("{error:#}");
            assert!(!rendered.contains("sentinel.invalid"));
            assert!(!rendered.contains("SENTINEL"));
            assert!(!rendered.contains(&endpoint));
        }
    }

    #[test]
    fn mcp_http_and_parse_failures_are_closed_and_use_the_retry_matrix() {
        for (status, body, expected_reason, retryable) in [
            (
                "400 Bad Request",
                "SENTINEL https://sentinel.invalid/private",
                "server_rejected",
                false,
            ),
            (
                "429 Too Many Requests",
                "SENTINEL https://sentinel.invalid/private",
                "rate_limited",
                true,
            ),
            (
                "503 Service Unavailable",
                "SENTINEL https://sentinel.invalid/private",
                "server_unavailable",
                true,
            ),
            (
                "200 OK",
                "SENTINEL invalid json https://sentinel.invalid/private",
                "invalid_json",
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let _request = read_http_request(&mut stream);
                write_http_text(&mut stream, status, body);
            });
            let error = McpClient::new("fixture-access", &endpoint).err().unwrap();
            server.join().unwrap();
            let failure = granola_failure_info(&error).unwrap();
            assert_eq!(failure.reason, expected_reason);
            assert_eq!(failure.retryable, retryable);
            let rendered = format!("{error:#}");
            assert!(!rendered.contains("sentinel.invalid"));
            assert!(!rendered.contains("SENTINEL"));
            assert!(!rendered.contains(&endpoint));
        }
    }

    #[test]
    fn dynamic_registration_uses_supported_public_authorization_code_metadata() {
        let redirect_uri = "http://127.0.0.1:49152/oauth2/callback";
        let body = registration_request(redirect_uri);
        assert_eq!(
            body["grant_types"],
            json!(["authorization_code", "refresh_token"])
        );
        assert_eq!(body["response_types"], json!(["code"]));
        assert_eq!(body["redirect_uris"], json!([redirect_uri]));
        assert_eq!(body["token_endpoint_auth_method"], "none");
        assert_eq!(body["scope"], REQUIRED_SCOPES.join(" "));
        assert!(!body.to_string().contains("device_code"));
        assert!(body.get("client_secret").is_none());
    }

    #[test]
    fn dynamic_registration_round_trips_against_hermetic_contract_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_json(&mut stream);
            assert_eq!(
                request["grant_types"],
                json!(["authorization_code", "refresh_token"])
            );
            assert_eq!(request["response_types"], json!(["code"]));
            assert_eq!(request["token_endpoint_auth_method"], "none");
            assert_eq!(
                request["redirect_uris"],
                json!(["http://127.0.0.1:49152/oauth2/callback"])
            );
            write_http_json(
                &mut stream,
                "201 Created",
                Some(json!({"client_id":"fixture-public-client"})),
            );
        });
        let endpoints = AuthEndpoints {
            authorize: "https://provider.invalid/authorize".into(),
            token: "https://provider.invalid/token".into(),
            register: endpoint,
            userinfo: "https://provider.invalid/userinfo".into(),
        };
        assert_eq!(
            register_client(&endpoints, "http://127.0.0.1:49152/oauth2/callback").unwrap(),
            "fixture-public-client"
        );
        server.join().unwrap();
    }

    #[test]
    fn registration_rejection_retryability_distinguishes_contract_and_transient_failures() {
        for (status, standard, reason, retryable) in [
            (422, None, "invalid_client_metadata", false),
            (
                400,
                Some("invalid_redirect_uri"),
                "invalid_redirect_uri",
                false,
            ),
            (429, None, "rate_limited", true),
            (503, None, "server_unavailable", true),
        ] {
            assert_eq!(classify_registration_failure(status, standard), reason);
            assert_eq!(
                GranolaNativeError::OAuthStage {
                    stage: "client_registration",
                    reason,
                }
                .retryable(),
                retryable
            );
        }
    }

    #[test]
    fn headless_callback_requires_exact_redirect_and_state() {
        let redirect = "http://127.0.0.1:49152/oauth2/callback";
        assert_eq!(
            validate_callback_url(
                "http://127.0.0.1:49152/oauth2/callback?code=fixture&state=expected",
                redirect,
                "expected"
            )
            .unwrap(),
            "fixture"
        );
        for callback in [
            "http://127.0.0.1:49153/oauth2/callback?code=fixture&state=expected",
            "http://127.0.0.1:49152/oauth2/callback?code=fixture&state=wrong",
            "http://127.0.0.1:49152/oauth2/callback?state=expected",
        ] {
            assert!(validate_callback_url(callback, redirect, "expected").is_err());
        }
    }

    #[test]
    #[ignore = "requires an explicitly isolated live Granola credential"]
    fn live_mcp_contract_probe_emits_only_redacted_structure() {
        if std::env::var("MARGINS_GRANOLA_LIVE_CONTRACT_PROBE").as_deref() != Ok("1") {
            return;
        }
        let margins_home = std::env::var_os("MARGINS_HOME")
            .map(PathBuf::from)
            .expect("MARGINS_HOME must select the isolated live fixture");
        let accounts = list_granola_accounts(&margins_home).unwrap();
        assert_eq!(accounts.len(), 1, "expected one isolated Granola account");
        let access_token = GranolaAccountStore::new(&margins_home, &accounts[0])
            .unwrap()
            .ensure_access_token()
            .unwrap();
        let mut client = McpClient::new(&access_token, GRANOLA_MCP_URL).unwrap();
        let result = client.request("tools/list", json!({})).unwrap();
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut summaries = tools
            .iter()
            .filter_map(redacted_tool_contract)
            .collect::<Vec<_>>();
        summaries.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
        let list_payload = client.call_tool("list_meetings", json!({})).unwrap();
        println!(
            "{}",
            json!({
                "tool_count": summaries.len(),
                "tools": summaries,
                "default_list": {
                    "extracted_count": extract_meeting_values(&list_payload).len(),
                    "payload_shape": redacted_payload_shape(&list_payload),
                },
            })
        );
    }

    fn redacted_tool_contract(tool: &Value) -> Option<Value> {
        let name = tool.get("name")?.as_str()?;
        let schema = tool.get("inputSchema").or_else(|| tool.get("input_schema"));
        let mut properties = schema
            .and_then(|value| value.get("properties"))
            .and_then(Value::as_object)
            .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        properties.sort();
        let mut required = schema
            .and_then(|value| value.get("required"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        required.sort();
        let mut time_range_values = schema
            .and_then(|value| value.get("properties"))
            .and_then(|value| value.get("time_range"))
            .and_then(|value| value.get("enum"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        time_range_values.sort();
        Some(json!({
            "name": name,
            "properties": properties,
            "required": required,
            "time_range_values": time_range_values,
        }))
    }

    fn redacted_payload_shape(value: &Value) -> Value {
        match value {
            Value::Null => json!({"type":"null"}),
            Value::Bool(_) => json!({"type":"boolean"}),
            Value::Number(_) => json!({"type":"number"}),
            Value::String(text) => {
                let trimmed = text.trim();
                json!({
                    "type":"string",
                    "bytes":text.len(),
                    "lines":text.lines().count(),
                    "starts_xml":trimmed.starts_with('<'),
                    "starts_json_object":trimmed.starts_with('{'),
                    "starts_json_array":trimmed.starts_with('['),
                    "starts_markdown_fence":trimmed.starts_with("```"),
                    "json_parseable":serde_json::from_str::<Value>(trimmed).is_ok(),
                    "xml_meeting_tag_count":trimmed.matches("<meeting").count(),
                    "xml_meetings_root_count":trimmed.matches("<meetings").count(),
                    "xml_meeting_with_attributes_count":trimmed.matches("<meeting ").count(),
                    "xml_plain_meeting_count":trimmed.matches("<meeting>").count(),
                    "xml_meeting_close_count":trimmed.matches("</meeting>").count(),
                    "xml_known_participants_count":trimmed.matches("<known_participants>").count(),
                    "xml_summary_count":trimmed.matches("<summary>").count(),
                })
            }
            Value::Array(values) => json!({"type":"array", "length":values.len()}),
            Value::Object(values) => {
                json!({"type":"object", "keys":values.keys().collect::<Vec<_>>()})
            }
        }
    }

    #[test]
    fn live_probe_redacts_provider_values_and_descriptions() {
        let tool = json!({
            "name":"list_meetings",
            "description":"provider-private description",
            "inputSchema":{"properties":{"time_range":{"type":"string","description":"private"}}},
        });
        let contract = redacted_tool_contract(&tool).unwrap().to_string();
        let payload = redacted_payload_shape(&Value::String(
            "provider-private meeting content".to_string(),
        ))
        .to_string();
        assert!(!contract.contains("provider-private"));
        assert!(!payload.contains("provider-private"));
    }
}
