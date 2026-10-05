//! Native Google Workspace OAuth and REST transport.
//!
//! This module is intentionally narrow: it knows the exact REST calls Margins
//! needs, and hands normalized records back to the existing connector
//! materializers. It does not provide a generic Google provider registry.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope,
    TokenResponse, TokenUrl,
};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::email::{parse_gmail_thread_from_value, GmailSearchPage, GmailThreadTransport};
use super::google_calendar::{
    CalendarAuthState, CalendarEvent, CalendarFetchBatch, CalendarFetchRequest, CalendarTransport,
};
use super::google_meet::{
    CalendarAttendeeEvidence, DriveTranscriptDocument, GoogleMeetSnapshot, GoogleMeetTransport,
    MeetConferenceRecord, MeetParticipantEvidence, MeetTranscriptMetadata,
};
use super::types::HealthStatus;
use crate::workspace::{google_account_dir, normalize_google_account};

const SERVICE_NAME: &str = "margins.google";
const TOKEN_KEYRING_LABEL: &str = "workspace-token-cache";
const TOKEN_FILE_NAME: &str = "token-cache.json";
const METADATA_FILE_NAME: &str = "account.json";
const STORAGE_SCHEMA: &str = "margins.google-token-cache.v1";
const METADATA_SCHEMA: &str = "margins.google-account.v1";
const API_RETRY_COUNT: usize = 5;
const API_MAX_RETRY_AFTER: Duration = Duration::from_secs(5);
type GoogleOAuthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

pub const GMAIL_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.readonly";
pub const CALENDAR_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
pub const DRIVE_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
pub const DOCS_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/documents.readonly";
pub const MEET_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/meetings.space.readonly";

pub const REQUIRED_GOOGLE_SCOPES: &[&str] = &[
    GMAIL_READONLY_SCOPE,
    CALENDAR_READONLY_SCOPE,
    DRIVE_READONLY_SCOPE,
    DOCS_READONLY_SCOPE,
    MEET_READONLY_SCOPE,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoogleCredentialBackendKind {
    #[serde(rename = "os_keyring")]
    OsKeyring,
    #[serde(rename = "file_0600", alias = "file0600")]
    File0600,
}

impl GoogleCredentialBackendKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OsKeyring => "os_keyring",
            Self::File0600 => "file_0600",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoogleNativeError {
    AccountMismatch {
        expected: String,
        actual: String,
    },
    ScopeMismatch {
        missing: Vec<String>,
    },
    StoragePermission {
        path: PathBuf,
        mode: u32,
    },
    CredentialBackendMismatch {
        requested: GoogleCredentialBackendKind,
        persisted: GoogleCredentialBackendKind,
    },
    CredentialsUnavailable(String),
    Quota {
        status: u16,
        retry_after: Option<String>,
    },
    Http {
        status: u16,
        message: String,
    },
    OAuth(String),
}

impl GoogleNativeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::AccountMismatch { .. } => "google_account_mismatch",
            Self::ScopeMismatch { .. } => "google_scope_mismatch",
            Self::StoragePermission { .. } => "google_storage_permission_insecure",
            Self::CredentialBackendMismatch { .. } => "google_credential_backend_mismatch",
            Self::CredentialsUnavailable(_) => "google_credentials_unavailable",
            Self::Quota { .. } => "google_quota_retry_exhausted",
            Self::Http { .. } => "google_http_failed",
            Self::OAuth(_) => "google_oauth_failed",
        }
    }
}

impl std::fmt::Display for GoogleNativeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AccountMismatch { expected, actual } => write!(
                formatter,
                "Google account mismatch: expected {expected}, authorized {actual}"
            ),
            Self::ScopeMismatch { missing } => {
                write!(
                    formatter,
                    "Google connection is missing required scopes: {}",
                    missing.join(", ")
                )
            }
            Self::StoragePermission { path, mode } => write!(
                formatter,
                "Google file credential store has insecure permissions at {}: mode {mode:o}",
                path.display()
            ),
            Self::CredentialBackendMismatch {
                requested,
                persisted,
            } => write!(
                formatter,
                "Google credential storage owner mismatch (requested {}, persisted {})",
                requested.as_str(),
                persisted.as_str()
            ),
            Self::CredentialsUnavailable(detail) => {
                write!(formatter, "Google credentials unavailable: {detail}")
            }
            Self::Quota {
                status,
                retry_after,
            } => write!(
                formatter,
                "Google quota retry exhausted after HTTP {status}{}",
                retry_after
                    .as_deref()
                    .map(|value| format!("; retry-after={value}"))
                    .unwrap_or_default()
            ),
            Self::Http { status, message } => write!(formatter, "Google HTTP {status}: {message}"),
            Self::OAuth(message) => write!(formatter, "Google OAuth failed: {message}"),
        }
    }
}

impl std::error::Error for GoogleNativeError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleConnectionMetadata {
    pub schema_version: String,
    pub account: String,
    pub storage: GoogleCredentialBackendKind,
    pub scopes: Vec<String>,
    pub connected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GoogleConnectionReady {
    pub account: String,
    pub storage: GoogleCredentialBackendKind,
    pub scopes: Vec<String>,
    pub access: Vec<&'static str>,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailHistoryDeletions {
    pub deleted_thread_ids: Vec<String>,
    pub next_history_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum GoogleOAuthMode {
    BrowserLoopback,
    HeadlessLoopback { redirect_uri: String },
}

pub trait GoogleOAuthPresenter: Send + Sync {
    fn present_authorization_url(
        &self,
        url: &str,
        need_code: bool,
    ) -> std::result::Result<String, String>;
    fn redirect_uri(&self) -> Option<&str> {
        None
    }
}

#[derive(Debug, Clone)]
pub struct GoogleAccountStore {
    account: String,
    account_dir: PathBuf,
    backend: GoogleCredentialBackendKind,
}

impl GoogleAccountStore {
    pub fn new(margins_home: &Path, account: &str) -> Result<Self> {
        Self::new_with_backend(margins_home, account, select_backend())
    }

    pub fn new_with_backend(
        margins_home: &Path,
        account: &str,
        backend: GoogleCredentialBackendKind,
    ) -> Result<Self> {
        let account = normalize_google_account(account)?;
        let account_dir = google_account_dir(margins_home, &account)?;
        Ok(Self {
            account,
            account_dir,
            backend,
        })
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    pub fn account_dir(&self) -> &Path {
        &self.account_dir
    }

    pub fn backend(&self) -> GoogleCredentialBackendKind {
        self.backend.clone()
    }

    pub fn token_storage(&self) -> MarginsTokenStorage {
        MarginsTokenStorage {
            account: self.account.clone(),
            account_dir: self.account_dir.clone(),
            backend: self.backend.clone(),
        }
    }

    pub fn metadata(&self) -> Result<Option<GoogleConnectionMetadata>> {
        let path = self.metadata_path();
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).with_context(|| {
            format!(
                "failed to read Google connection metadata {}",
                path.display()
            )
        })?;
        let metadata: GoogleConnectionMetadata =
            serde_json::from_slice(&bytes).context("failed to parse Google connection metadata")?;
        Ok(Some(metadata))
    }

    /// Check credential-store ownership using account metadata only.
    ///
    /// A historical `os_keyring` record does not say whether an older CLI or
    /// the desktop app created it. A File0600 caller must therefore refuse it:
    /// migrating it would require crossing the Keychain boundary that this
    /// preflight exists to protect.
    pub fn preflight_backend_ownership(&self) -> Result<()> {
        self.ensure_backend_ownership()
    }

    pub fn durable_connection_metadata(&self, scopes: &[&str]) -> Result<GoogleConnectionMetadata> {
        let metadata = self.metadata()?.ok_or_else(|| {
            GoogleNativeError::CredentialsUnavailable("missing Google account metadata".into())
        })?;
        if metadata.account != self.account {
            return Err(GoogleNativeError::AccountMismatch {
                expected: self.account.clone(),
                actual: metadata.account,
            }
            .into());
        }
        self.ensure_backend_matches(&metadata)?;
        let missing = missing_scopes(&metadata.scopes, scopes);
        if !missing.is_empty() {
            return Err(GoogleNativeError::ScopeMismatch { missing }.into());
        }
        let storage = self.token_storage();
        let token = storage.cached_token_for(scopes)?.ok_or_else(|| {
            GoogleNativeError::CredentialsUnavailable(
                "no stored Google token covers the required scopes".into(),
            )
        })?;
        ensure_refreshable_token(&token)?;
        Ok(metadata)
    }

    pub fn write_metadata(&self, scopes: &[&str]) -> Result<GoogleConnectionMetadata> {
        self.ensure_backend_ownership()?;
        ensure_private_dir_chain(&self.account_dir)?;
        let metadata = GoogleConnectionMetadata {
            schema_version: METADATA_SCHEMA.to_string(),
            account: self.account.clone(),
            storage: self.backend.clone(),
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            connected_at: Utc::now(),
        };
        write_private_json(&self.metadata_path(), &metadata)?;
        Ok(metadata)
    }

    pub fn forget(&self) -> Result<()> {
        self.ensure_backend_ownership()?;
        self.token_storage().delete()?;
        if self.account_dir.exists() {
            std::fs::remove_dir_all(&self.account_dir).with_context(|| {
                format!(
                    "failed to remove Google account directory {}",
                    self.account_dir.display()
                )
            })?;
        }
        Ok(())
    }

    fn metadata_path(&self) -> PathBuf {
        self.account_dir.join(METADATA_FILE_NAME)
    }

    fn ensure_backend_ownership(&self) -> Result<()> {
        if let Some(metadata) = self.metadata()? {
            self.ensure_backend_matches(&metadata)?;
        }
        Ok(())
    }

    fn ensure_backend_matches(&self, metadata: &GoogleConnectionMetadata) -> Result<()> {
        if metadata.storage != self.backend {
            return Err(GoogleNativeError::CredentialBackendMismatch {
                requested: self.backend.clone(),
                persisted: metadata.storage.clone(),
            }
            .into());
        }
        Ok(())
    }
}

fn select_backend() -> GoogleCredentialBackendKind {
    match std::env::var("MARGINS_GOOGLE_CREDENTIAL_BACKEND")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "file" | "file_0600" | "headless_file" => GoogleCredentialBackendKind::File0600,
        "keyring" | "os_keyring" => GoogleCredentialBackendKind::OsKeyring,
        _ => GoogleCredentialBackendKind::OsKeyring,
    }
}

#[derive(Debug, Clone)]
pub struct MarginsTokenStorage {
    account: String,
    account_dir: PathBuf,
    backend: GoogleCredentialBackendKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokenEnvelope {
    schema_version: String,
    account: String,
    tokens: Vec<StoredScopedToken>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredScopedToken {
    scopes: Vec<String>,
    token: StoredOAuthToken,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredOAuthToken {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

impl StoredOAuthToken {
    fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now() + chrono::Duration::seconds(60))
    }
}

fn ensure_refreshable_token(token: &StoredOAuthToken) -> Result<()> {
    if token
        .refresh_token
        .as_deref()
        .is_some_and(|refresh_token| !refresh_token.trim().is_empty())
    {
        return Ok(());
    }
    Err(GoogleNativeError::CredentialsUnavailable(
        "stored Google token has no refresh token for unattended access".into(),
    )
    .into())
}

impl MarginsTokenStorage {
    fn snapshot_envelope(&self) -> Result<Option<StoredTokenEnvelope>> {
        if self.backend == GoogleCredentialBackendKind::File0600 && !self.file_path().exists() {
            return Ok(None);
        }
        match self.load_envelope() {
            Ok(envelope) => Ok(Some(envelope)),
            Err(error)
                if self.backend == GoogleCredentialBackendKind::OsKeyring
                    && ["no entry", "not found"].iter().any(|needle| {
                        format!("{error:#}").to_ascii_lowercase().contains(needle)
                    }) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn store_token_for(&self, scopes: &[&str], token: StoredOAuthToken) -> Result<()> {
        let mut envelope = self
            .load_envelope()
            .unwrap_or_else(|_| StoredTokenEnvelope {
                schema_version: STORAGE_SCHEMA.to_string(),
                account: self.account.clone(),
                tokens: Vec::new(),
            });
        let mut scopes = scopes
            .iter()
            .map(|scope| (*scope).to_string())
            .collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        envelope.tokens.retain(|entry| entry.scopes != scopes);
        envelope.tokens.push(StoredScopedToken { scopes, token });
        self.store_envelope(&envelope)
    }

    fn cached_token_for(&self, scopes: &[&str]) -> Result<Option<StoredOAuthToken>> {
        let envelope = match self.load_envelope() {
            Ok(envelope) => envelope,
            Err(error)
                if error
                    .downcast_ref::<GoogleNativeError>()
                    .is_some_and(|error| {
                        matches!(error, GoogleNativeError::CredentialsUnavailable(_))
                    }) =>
            {
                return Ok(None);
            }
            Err(error)
                if format!("{error:#}")
                    .to_ascii_lowercase()
                    .contains("no entry") =>
            {
                return Ok(None);
            }
            Err(error)
                if format!("{error:#}")
                    .to_ascii_lowercase()
                    .contains("not found") =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut requested = scopes.to_vec();
        requested.sort_unstable();
        requested.dedup();
        Ok(envelope
            .tokens
            .into_iter()
            .find(|entry| {
                requested
                    .iter()
                    .all(|scope| entry.scopes.iter().any(|candidate| candidate == scope))
            })
            .map(|entry| entry.token))
    }

    pub fn delete(&self) -> Result<()> {
        match self.backend {
            GoogleCredentialBackendKind::OsKeyring => {
                let entry =
                    keyring::Entry::new(SERVICE_NAME, &self.keyring_user()).map_err(|error| {
                        GoogleNativeError::CredentialsUnavailable(error.to_string())
                    })?;
                match entry.delete_credential() {
                    Ok(()) => Ok(()),
                    Err(error) if error.to_string().to_ascii_lowercase().contains("no entry") => {
                        Ok(())
                    }
                    Err(error) => {
                        Err(GoogleNativeError::CredentialsUnavailable(error.to_string()).into())
                    }
                }
            }
            GoogleCredentialBackendKind::File0600 => {
                let path = self.file_path();
                if path.exists() {
                    std::fs::remove_file(&path).with_context(|| {
                        format!(
                            "failed to remove Google file token store {}",
                            path.display()
                        )
                    })?;
                }
                Ok(())
            }
        }
    }

    fn load_envelope(&self) -> Result<StoredTokenEnvelope> {
        let secret = match self.backend {
            GoogleCredentialBackendKind::OsKeyring => {
                let entry =
                    keyring::Entry::new(SERVICE_NAME, &self.keyring_user()).map_err(|error| {
                        GoogleNativeError::CredentialsUnavailable(error.to_string())
                    })?;
                entry
                    .get_password()
                    .map_err(|error| GoogleNativeError::CredentialsUnavailable(error.to_string()))?
                    .into_bytes()
            }
            GoogleCredentialBackendKind::File0600 => {
                let path = self.file_path();
                if !path.exists() {
                    return Err(GoogleNativeError::CredentialsUnavailable(format!(
                        "Google file token store is missing at {}",
                        path.display()
                    ))
                    .into());
                }
                validate_private_file_permissions(&path)?;
                std::fs::read(&path).with_context(|| {
                    format!("failed to read Google file token store {}", path.display())
                })?
            }
        };
        let envelope: StoredTokenEnvelope =
            serde_json::from_slice(&secret).context("failed to parse Google token store")?;
        if envelope.schema_version != STORAGE_SCHEMA || envelope.account != self.account {
            bail!("Google token store identity is invalid");
        }
        Ok(envelope)
    }

    fn store_envelope(&self, envelope: &StoredTokenEnvelope) -> Result<()> {
        match self.backend {
            GoogleCredentialBackendKind::OsKeyring => {
                let entry =
                    keyring::Entry::new(SERVICE_NAME, &self.keyring_user()).map_err(|error| {
                        GoogleNativeError::CredentialsUnavailable(error.to_string())
                    })?;
                let secret = serde_json::to_string(envelope)?;
                entry.set_password(&secret).map_err(|error| {
                    GoogleNativeError::CredentialsUnavailable(error.to_string())
                })?;
                Ok(())
            }
            GoogleCredentialBackendKind::File0600 => {
                ensure_private_dir_chain(&self.account_dir)?;
                write_private_json(&self.file_path(), envelope)
            }
        }
    }

    fn file_path(&self) -> PathBuf {
        self.account_dir.join(TOKEN_FILE_NAME)
    }

    fn keyring_user(&self) -> String {
        format!("{}:{TOKEN_KEYRING_LABEL}", self.account)
    }
}

#[derive(Debug, Clone, Default)]
struct StagedTokenStorage {
    tokens: Arc<Mutex<Vec<StoredScopedToken>>>,
}

impl StagedTokenStorage {
    fn set(&self, scopes: &[&str], token: StoredOAuthToken) -> Result<()> {
        let mut scopes = scopes
            .iter()
            .map(|scope| (*scope).to_string())
            .collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        let mut guard = self
            .tokens
            .lock()
            .map_err(|_| anyhow!("staged token lock poisoned"))?;
        guard.retain(|entry| entry.scopes != scopes);
        guard.push(StoredScopedToken { scopes, token });
        Ok(())
    }

    fn token_for(&self, scopes: &[&str]) -> Option<StoredOAuthToken> {
        let mut requested = scopes.to_vec();
        requested.sort_unstable();
        requested.dedup();
        self.tokens
            .lock()
            .ok()?
            .iter()
            .find(|entry| {
                requested
                    .iter()
                    .all(|scope| entry.scopes.iter().any(|candidate| candidate == scope))
            })
            .map(|entry| entry.token.clone())
    }
}

#[cfg(unix)]
pub fn validate_private_file_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !path.exists() {
        return Ok(());
    }
    let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(GoogleNativeError::StoragePermission {
            path: path.to_path_buf(),
            mode,
        }
        .into());
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn validate_private_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    write_private_bytes(path, &bytes)
}

fn write_private_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_private_dir_chain(parent)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open private file {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write private file {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn ensure_private_dir_chain(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("failed to create private directory {}", path.display()))?;
    set_private_dir_permissions(path)?;
    if let Some(parent) = path.parent().filter(|parent| {
        parent
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("google"))
    }) {
        set_private_dir_permissions(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict private directory {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

pub fn connect_google_account(
    margins_home: &Path,
    expected_account: Option<&str>,
    backend: GoogleCredentialBackendKind,
    credential_json: &[u8],
    mode: GoogleOAuthMode,
    presenter: Arc<dyn GoogleOAuthPresenter>,
) -> Result<GoogleConnectionReady> {
    if let Some(account) = expected_account {
        GoogleAccountStore::new_with_backend(margins_home, account, backend.clone())?
            .preflight_backend_ownership()?;
    }
    let staged_storage = StagedTokenStorage::default();
    let token = run_installed_oauth(credential_json, mode, presenter)?;
    let access_token = token.access_token.clone();
    staged_storage.set(REQUIRED_GOOGLE_SCOPES, token)?;
    let client = NativeGoogleClient::with_access_token(access_token);
    let account = client.verified_account(expected_account)?;
    let store = GoogleAccountStore::new_with_backend(margins_home, &account, backend.clone())?;
    // With no expected account, the OAuth identity is unknowable until after
    // consent and the read-only profile lookup. This is the earliest safe
    // ownership check, and it still runs before the remaining capability probes.
    store.preflight_backend_ownership()?;
    let ready = client.verify_capabilities(account, backend)?;
    commit_verified_google_connection(&store, &ready, &staged_storage)
}

fn commit_verified_google_connection(
    store: &GoogleAccountStore,
    ready: &GoogleConnectionReady,
    staged_storage: &StagedTokenStorage,
) -> Result<GoogleConnectionReady> {
    let expected = store.account().to_string();
    if ready.account != expected {
        return Err(GoogleNativeError::AccountMismatch {
            expected,
            actual: ready.account.clone(),
        }
        .into());
    }
    let missing = missing_scopes(&ready.scopes, REQUIRED_GOOGLE_SCOPES);
    if !missing.is_empty() {
        return Err(GoogleNativeError::ScopeMismatch { missing }.into());
    }
    let token = staged_storage
        .token_for(REQUIRED_GOOGLE_SCOPES)
        .ok_or_else(|| {
            GoogleNativeError::CredentialsUnavailable(
                "OAuth did not return a refreshable stored token".into(),
            )
        })?;
    ensure_refreshable_token(&token)?;
    store.ensure_backend_ownership()?;
    let storage = store.token_storage();
    let previous_tokens = storage.snapshot_envelope()?;
    let previous_metadata = if store.metadata_path().exists() {
        Some(std::fs::read(store.metadata_path()).with_context(|| {
            format!(
                "failed to snapshot Google connection metadata {}",
                store.metadata_path().display()
            )
        })?)
    } else {
        None
    };
    storage.store_token_for(REQUIRED_GOOGLE_SCOPES, token)?;
    if let Err(error) = store.write_metadata(REQUIRED_GOOGLE_SCOPES) {
        if let Some(previous_tokens) = previous_tokens.as_ref() {
            let _ = storage.store_envelope(previous_tokens);
        } else {
            let _ = storage.delete();
        }
        if let Some(previous_metadata) = previous_metadata.as_deref() {
            let _ = write_private_bytes(&store.metadata_path(), previous_metadata);
        } else {
            let _ = std::fs::remove_file(store.metadata_path());
        }
        return Err(error);
    }
    Ok(ready.clone())
}

#[derive(Debug, Deserialize)]
struct GoogleOAuthSecret {
    installed: Option<GoogleOAuthSecretInner>,
    web: Option<GoogleOAuthSecretInner>,
}

#[derive(Debug, Deserialize)]
struct GoogleOAuthSecretInner {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

#[derive(Debug)]
struct OAuthClientConfig {
    client_id: String,
    client_secret: String,
    auth_uri: String,
    token_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OAuthCallback {
    code: String,
    state: String,
}

fn parse_oauth_secret(credential_json: &[u8]) -> Result<OAuthClientConfig> {
    let parsed: GoogleOAuthSecret = serde_json::from_slice(credential_json)
        .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&error.to_string())))?;
    let inner = parsed.installed.or(parsed.web).ok_or_else(|| {
        GoogleNativeError::OAuth("Google OAuth credential lacks installed/web client".into())
    })?;
    Ok(OAuthClientConfig {
        client_id: inner.client_id,
        client_secret: inner.client_secret,
        auth_uri: inner.auth_uri,
        token_uri: inner.token_uri,
    })
}

fn oauth_client(config: &OAuthClientConfig, redirect_uri: &str) -> Result<GoogleOAuthClient> {
    Ok(BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_client_secret(ClientSecret::new(config.client_secret.clone()))
        .set_auth_uri(
            AuthUrl::new(config.auth_uri.clone())
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?,
        )
        .set_token_uri(
            TokenUrl::new(config.token_uri.clone())
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?,
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.to_string())
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?,
        )
        .set_auth_type(AuthType::RequestBody))
}

fn oauth_http_client() -> Result<oauth2::reqwest::blocking::Client> {
    oauth2::reqwest::blocking::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&error.to_string())).into())
}

fn run_installed_oauth(
    credential_json: &[u8],
    mode: GoogleOAuthMode,
    presenter: Arc<dyn GoogleOAuthPresenter>,
) -> Result<StoredOAuthToken> {
    let config = parse_oauth_secret(credential_json)?;
    let timeout = oauth_timeout();
    let (redirect_uri, listener) = match &mode {
        GoogleOAuthMode::BrowserLoopback => {
            let listener = TcpListener::bind("127.0.0.1:0")
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
            let port = listener
                .local_addr()
                .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?
                .port();
            (
                format!("http://127.0.0.1:{port}/oauth2/callback"),
                Some(listener),
            )
        }
        GoogleOAuthMode::HeadlessLoopback { redirect_uri } => {
            let redirect_uri = if redirect_uri.trim().is_empty() {
                headless_redirect_uri_with_ephemeral_port()?
            } else {
                redirect_uri.clone()
            };
            (redirect_uri, None)
        }
    };
    let expected = ExpectedCallback::from_redirect_uri(&redirect_uri)?;
    let client = oauth_client(&config, &redirect_uri)?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let scopes = REQUIRED_GOOGLE_SCOPES
        .iter()
        .map(|scope| Scope::new((*scope).to_string()));
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        .add_scopes(scopes)
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .set_pkce_challenge(pkce_challenge)
        .url();
    assert_authorization_url_uses_pkce(&auth_url)?;
    let callback = match listener {
        Some(listener) => {
            presenter
                .present_authorization_url(auth_url.as_str(), false)
                .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&error)))?;
            listen_for_oauth_callback(listener, &expected, csrf_token.secret(), timeout)?
        }
        None => {
            let callback = presenter
                .present_authorization_url(auth_url.as_str(), true)
                .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&error)))?;
            validate_oauth_callback(&callback, &expected, csrf_token.secret())?
        }
    };
    exchange_oauth_code(&client, callback, pkce_verifier)
}

fn headless_redirect_uri_with_ephemeral_port() -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?
        .port();
    Ok(format!("http://127.0.0.1:{port}/oauth2/callback"))
}

fn oauth_timeout() -> Duration {
    std::env::var("MARGINS_GOOGLE_OAUTH_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(300))
        .min(Duration::from_secs(900))
}

#[derive(Debug, Clone)]
struct ExpectedCallback {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
}

impl ExpectedCallback {
    fn from_redirect_uri(redirect_uri: &str) -> Result<Self> {
        let url = url::Url::parse(redirect_uri)
            .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
        Ok(Self {
            scheme: url.scheme().to_string(),
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default(),
            path: url.path().to_string(),
        })
    }
}

fn validate_oauth_callback(
    callback: &str,
    expected: &ExpectedCallback,
    expected_state: &str,
) -> Result<OAuthCallback> {
    let url = url::Url::parse(callback)
        .map_err(|_| GoogleNativeError::OAuth("invalid OAuth callback URL".into()))?;
    let actual_host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if url.scheme() != expected.scheme
        || actual_host != expected.host
        || url.port_or_known_default() != expected.port
        || url.path() != expected.path
    {
        return Err(GoogleNativeError::OAuth(
            "OAuth callback did not match the expected loopback redirect".into(),
        )
        .into());
    }
    let mut code = None;
    let mut state = None;
    let mut provider_error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => provider_error = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = provider_error {
        return Err(GoogleNativeError::OAuth(format!(
            "OAuth provider returned an error: {}",
            redact_secret_text(&error)
        ))
        .into());
    }
    let state =
        state.ok_or_else(|| GoogleNativeError::OAuth("OAuth callback missing state".into()))?;
    if state != expected_state {
        return Err(GoogleNativeError::OAuth("OAuth callback state mismatch".into()).into());
    }
    let code =
        code.ok_or_else(|| GoogleNativeError::OAuth("OAuth callback missing code".into()))?;
    Ok(OAuthCallback { code, state })
}

fn listen_for_oauth_callback(
    listener: TcpListener,
    expected: &ExpectedCallback,
    expected_state: &str,
    timeout: Duration,
) -> Result<OAuthCallback> {
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                // macOS/BSD accepted sockets inherit the listener's non-blocking
                // mode; a callback whose bytes have not arrived yet would fail
                // with WouldBlock. Read it in blocking mode with a bound.
                stream
                    .set_nonblocking(false)
                    .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                let mut buffer = [0_u8; 8192];
                let read = stream
                    .read(&mut buffer)
                    .map_err(|error| GoogleNativeError::OAuth(error.to_string()))?;
                let request = String::from_utf8_lossy(&buffer[..read]);
                let mut lines = request.lines();
                let first = lines.next().unwrap_or_default();
                let target = first.split_whitespace().nth(1).ok_or_else(|| {
                    GoogleNativeError::OAuth("OAuth callback request had no target".into())
                })?;
                let host = lines
                    .find_map(|line| {
                        line.strip_prefix("Host: ")
                            .or_else(|| line.strip_prefix("host: "))
                    })
                    .unwrap_or("127.0.0.1");
                let callback = format!("{}://{}{}", expected.scheme, host.trim(), target);
                let parsed = validate_oauth_callback(&callback, expected, expected_state);
                let body = if parsed.is_ok() {
                    "<!doctype html><meta charset=\"utf-8\"><title>Google connected</title><p>Google connection complete. Return to Margins.</p>"
                } else {
                    "<!doctype html><meta charset=\"utf-8\"><title>Google connection failed</title><p>Google connection failed validation. Return to Margins.</p>"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nPragma: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                return parsed;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(GoogleNativeError::OAuth("OAuth consent timed out".into()).into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(GoogleNativeError::OAuth(error.to_string()).into()),
        }
    }
}

fn assert_authorization_url_uses_pkce(auth_url: &url::Url) -> Result<()> {
    let mut has_challenge = false;
    let mut has_s256 = false;
    let mut has_state = false;
    for (key, value) in auth_url.query_pairs() {
        match key.as_ref() {
            "code_challenge" if !value.is_empty() => has_challenge = true,
            "code_challenge_method" if value == "S256" => has_s256 = true,
            "state" if !value.is_empty() => has_state = true,
            _ => {}
        }
    }
    if has_challenge && has_s256 && has_state {
        Ok(())
    } else {
        Err(
            GoogleNativeError::OAuth("OAuth authorization URL lacked PKCE S256 or state".into())
                .into(),
        )
    }
}

fn exchange_oauth_code(
    client: &GoogleOAuthClient,
    callback: OAuthCallback,
    pkce_verifier: PkceCodeVerifier,
) -> Result<StoredOAuthToken> {
    let http = oauth_http_client()?;
    let token = client
        .exchange_code(AuthorizationCode::new(callback.code))
        .set_pkce_verifier(pkce_verifier)
        .request(&http)
        .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&format!("{error:#}"))))?;
    token_response_to_stored(token)
}

fn token_response_to_stored<EF, TT>(
    token: oauth2::StandardTokenResponse<EF, TT>,
) -> Result<StoredOAuthToken>
where
    EF: oauth2::ExtraTokenFields,
    TT: oauth2::TokenType,
{
    let expires_at = token
        .expires_in()
        .and_then(|duration| chrono::Duration::from_std(duration).ok())
        .map(|duration| Utc::now() + duration);
    Ok(StoredOAuthToken {
        access_token: token.access_token().secret().to_string(),
        refresh_token: token
            .refresh_token()
            .map(|token| token.secret().to_string()),
        expires_at,
    })
}

fn missing_scopes(granted: &[String], required: &[&str]) -> Vec<String> {
    let granted = granted.iter().map(String::as_str).collect::<BTreeSet<_>>();
    required
        .iter()
        .copied()
        .filter(|scope| !granted.contains(scope))
        .map(str::to_string)
        .collect()
}

#[derive(Debug, Clone)]
pub struct GoogleTokenProvider {
    margins_home: PathBuf,
    account: String,
    credential_json: Vec<u8>,
    backend: GoogleCredentialBackendKind,
}

impl GoogleTokenProvider {
    pub fn new(margins_home: &Path, account: &str, credential_json: &[u8]) -> Result<Self> {
        Self::new_with_backend(
            margins_home,
            account,
            credential_json,
            GoogleCredentialBackendKind::OsKeyring,
        )
    }

    pub fn new_with_backend(
        margins_home: &Path,
        account: &str,
        credential_json: &[u8],
        backend: GoogleCredentialBackendKind,
    ) -> Result<Self> {
        Ok(Self {
            margins_home: margins_home.to_path_buf(),
            account: normalize_google_account(account)?,
            credential_json: credential_json.to_vec(),
            backend,
        })
    }

    fn access_token(&self, scopes: &[&str]) -> Result<StoredOAuthToken> {
        let store = GoogleAccountStore::new_with_backend(
            &self.margins_home,
            &self.account,
            self.backend.clone(),
        )?;
        store.durable_connection_metadata(scopes)?;
        let storage = MarginsTokenStorage {
            account: self.account.clone(),
            account_dir: store.account_dir.clone(),
            backend: self.backend.clone(),
        };
        let cached = storage.cached_token_for(scopes)?.ok_or_else(|| {
            GoogleNativeError::CredentialsUnavailable(
                "no stored Google token covers the requested scope".into(),
            )
        })?;
        if !cached.is_expired() {
            return Ok(cached);
        }
        let refresh_token = cached.refresh_token.clone().ok_or_else(|| {
            GoogleNativeError::CredentialsUnavailable(
                "stored Google token has no refresh token".into(),
            )
        })?;
        let config = parse_oauth_secret(&self.credential_json)?;
        let client = oauth_client(&config, "http://127.0.0.1/oauth2/refresh")?;
        let http = oauth_http_client()?;
        let response = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .add_scopes(scopes.iter().map(|scope| Scope::new((*scope).to_string())))
            .request(&http)
            .map_err(|error| GoogleNativeError::OAuth(redact_secret_text(&format!("{error:#}"))))?;
        let mut refreshed = token_response_to_stored(response)?;
        if refreshed.refresh_token.is_none() {
            refreshed.refresh_token = cached.refresh_token;
        }
        storage.store_token_for(scopes, refreshed.clone())?;
        if refreshed.access_token.trim().is_empty() {
            return Err(GoogleNativeError::CredentialsUnavailable(
                "refreshed Google token has no access token".into(),
            )
            .into());
        }
        Ok(refreshed)
    }

    pub fn bearer_token(&self, scopes: &[&str]) -> Result<String> {
        let token = self.access_token(scopes)?;
        Ok(token.access_token)
    }
}

#[derive(Debug, Clone)]
pub struct NativeGoogleClient {
    http: Client,
    token_provider: Option<GoogleTokenProvider>,
    access_token: Option<String>,
    token_cache: Arc<Mutex<BTreeMap<String, StoredOAuthToken>>>,
    gmail_base: String,
    calendar_base: String,
    drive_base: String,
    docs_base: String,
    meet_base: String,
}

impl NativeGoogleClient {
    pub fn new(token_provider: GoogleTokenProvider) -> Self {
        Self::with_bases(
            Some(token_provider),
            None,
            "https://gmail.googleapis.com",
            "https://www.googleapis.com/calendar",
            "https://www.googleapis.com/drive",
            "https://docs.googleapis.com",
            "https://meet.googleapis.com",
        )
    }

    pub fn with_access_token(access_token: String) -> Self {
        Self::with_bases(
            None,
            Some(access_token),
            "https://gmail.googleapis.com",
            "https://www.googleapis.com/calendar",
            "https://www.googleapis.com/drive",
            "https://docs.googleapis.com",
            "https://meet.googleapis.com",
        )
    }

    pub fn with_bases(
        token_provider: Option<GoogleTokenProvider>,
        access_token: Option<String>,
        gmail_base: impl Into<String>,
        calendar_base: impl Into<String>,
        drive_base: impl Into<String>,
        docs_base: impl Into<String>,
        meet_base: impl Into<String>,
    ) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(45))
                .build()
                .expect("Google HTTP client builds"),
            token_provider,
            access_token,
            token_cache: Arc::new(Mutex::new(BTreeMap::new())),
            gmail_base: gmail_base.into(),
            calendar_base: calendar_base.into(),
            drive_base: drive_base.into(),
            docs_base: docs_base.into(),
            meet_base: meet_base.into(),
        }
    }

    pub fn verify_connection(
        &self,
        expected_account: Option<&str>,
        storage: GoogleCredentialBackendKind,
    ) -> Result<GoogleConnectionReady> {
        let actual = self.verified_account(expected_account)?;
        self.verify_capabilities(actual, storage)
    }

    fn verified_account(&self, expected_account: Option<&str>) -> Result<String> {
        let profile = self
            .get_json(
                GMAIL_READONLY_SCOPE,
                &format!("{}/gmail/v1/users/me/profile", self.gmail_base),
                &[],
            )
            .context("failed to verify Gmail read-only access")?;
        let actual = profile
            .get("emailAddress")
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase)
            .context("Gmail profile response did not include emailAddress")?;
        if let Some(expected_account) = expected_account {
            let expected = normalize_google_account(expected_account)?;
            if actual != expected {
                return Err(GoogleNativeError::AccountMismatch { expected, actual }.into());
            }
        }

        let actual = normalize_google_account(&actual)?;

        Ok(actual)
    }

    fn verify_capabilities(
        &self,
        actual: String,
        storage: GoogleCredentialBackendKind,
    ) -> Result<GoogleConnectionReady> {
        self.get_json(
            CALENDAR_READONLY_SCOPE,
            &format!("{}/v3/calendars/primary/events", self.calendar_base),
            &[("maxResults", "1"), ("singleEvents", "false")],
        )
        .context("failed to verify Calendar read-only access")?;
        self.get_json(
            DRIVE_READONLY_SCOPE,
            &format!("{}/v3/files", self.drive_base),
            &[("pageSize", "1"), ("fields", "files(id),nextPageToken")],
        )
        .context("failed to verify Drive read-only access")?;
        match self.get_json(
            DOCS_READONLY_SCOPE,
            &format!(
                "{}/v1/documents/__margins_capability_probe__",
                self.docs_base
            ),
            &[],
        ) {
            Ok(_) => {}
            Err(error) if format!("{error:#}").contains("HTTP 404") => {}
            Err(error) => return Err(error).context("failed to verify Docs read-only access"),
        }
        self.get_json(
            MEET_READONLY_SCOPE,
            &format!("{}/v2/conferenceRecords", self.meet_base),
            &[("pageSize", "1")],
        )
        .context("failed to verify Meet read-only access")?;

        Ok(GoogleConnectionReady {
            account: actual,
            storage,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).to_string())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        })
    }

    fn bearer_token(&self, scopes: &[&str]) -> Result<String> {
        let token = match &self.access_token {
            Some(token) => return Ok(token.clone()),
            None => {
                let key = token_cache_key(scopes);
                if let Some(token) = self
                    .token_cache
                    .lock()
                    .map_err(|_| anyhow!("Google token cache lock poisoned"))?
                    .get(&key)
                    .filter(|token| !token.is_expired())
                    .map(|token| token.access_token.clone())
                {
                    return Ok(token);
                }
                let token = self
                    .token_provider
                    .as_ref()
                    .context("Google token provider missing")?
                    .access_token(scopes)?;
                let access_token = token.access_token.clone();
                self.token_cache
                    .lock()
                    .map_err(|_| anyhow!("Google token cache lock poisoned"))?
                    .insert(key, token);
                access_token
            }
        };
        Ok(token)
    }

    fn request(
        &self,
        scope: &str,
        method: Method,
        url: &str,
        query: &[(&str, &str)],
    ) -> Result<RequestBuilder> {
        let token = self.bearer_token(&[scope])?;
        Ok(self
            .http
            .request(method, url)
            .bearer_auth(token)
            .query(query))
    }

    fn get_json(&self, scope: &str, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        self.send_json(|| self.request(scope, Method::GET, url, query))
    }

    pub fn gmail_history_deletions(
        &self,
        account: &str,
        start_history_id: &str,
    ) -> Result<GmailHistoryDeletions> {
        let mut page_token: Option<String> = None;
        let mut deleted_thread_ids = BTreeSet::new();
        let mut next_history_id = None;
        loop {
            let mut params = vec![
                ("startHistoryId", start_history_id),
                ("historyTypes", "messageDeleted"),
                ("maxResults", "500"),
            ];
            if let Some(page_token) = page_token.as_deref() {
                params.push(("pageToken", page_token));
            }
            let value = self.get_json(
                GMAIL_READONLY_SCOPE,
                &format!(
                    "{}/gmail/v1/users/{}/history",
                    self.gmail_base,
                    url_component(account)
                ),
                &params,
            )?;
            for history in value
                .get("history")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for deleted in history
                    .get("messagesDeleted")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(thread_id) = deleted
                        .get("message")
                        .and_then(|message| message.get("threadId"))
                        .and_then(Value::as_str)
                        .filter(|thread_id| !thread_id.trim().is_empty())
                    {
                        deleted_thread_ids.insert(thread_id.to_string());
                    }
                }
            }
            next_history_id = value
                .get("historyId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(next_history_id);
            page_token = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        Ok(GmailHistoryDeletions {
            deleted_thread_ids: deleted_thread_ids.into_iter().collect(),
            next_history_id,
        })
    }

    fn send_json<F>(&self, build: F) -> Result<Value>
    where
        F: Fn() -> Result<RequestBuilder>,
    {
        let mut last_quota = None;
        for attempt in 0..=API_RETRY_COUNT {
            let response = build()?.send().map_err(|error| GoogleNativeError::Http {
                status: 0,
                message: redact_secret_text(&error.to_string()),
            })?;
            let status = response.status();
            if status.is_success() {
                return response
                    .json::<Value>()
                    .context("Google API returned invalid JSON");
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let body = response.text().unwrap_or_default();
            if is_retryable_quota(status, &body) && attempt < API_RETRY_COUNT {
                last_quota = Some((status.as_u16(), retry_after.clone()));
                std::thread::sleep(retry_delay(attempt, retry_after.as_deref()));
                continue;
            }
            if is_retryable_quota(status, &body) {
                let (status, retry_after) = last_quota.unwrap_or((status.as_u16(), retry_after));
                return Err(GoogleNativeError::Quota {
                    status,
                    retry_after,
                }
                .into());
            }
            return Err(GoogleNativeError::Http {
                status: status.as_u16(),
                message: redact_secret_text(&body),
            }
            .into());
        }
        Err(anyhow!("unreachable Google retry loop"))
    }
}

fn token_cache_key(scopes: &[&str]) -> String {
    let mut scopes = scopes.to_vec();
    scopes.sort_unstable();
    scopes.dedup();
    scopes.join("\n")
}

fn is_retryable_quota(status: StatusCode, body: &str) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        return true;
    }
    if status != StatusCode::FORBIDDEN {
        return false;
    }
    let body = body.to_ascii_lowercase();
    [
        "rateLimitExceeded",
        "userratelimitexceeded",
        "quotaexceeded",
        "resource_exhausted",
    ]
    .iter()
    .any(|marker| body.contains(&marker.to_ascii_lowercase()))
}

fn retry_delay(attempt: usize, retry_after: Option<&str>) -> Duration {
    retry_after
        .and_then(parse_retry_after)
        .map(|duration| duration.min(API_MAX_RETRY_AFTER))
        .unwrap_or_else(|| Duration::from_millis(250 * (attempt as u64 + 1)))
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

impl GmailThreadTransport for NativeGoogleClient {
    fn search_page(
        &self,
        ctx: &super::types::ConnectorCtx,
        query: &str,
        max_results: u64,
        page_token: Option<&str>,
    ) -> Result<GmailSearchPage> {
        let max = max_results.to_string();
        let mut params = vec![
            ("q", query),
            ("maxResults", max.as_str()),
            ("fields", "threads/id,nextPageToken"),
        ];
        if let Some(page_token) = page_token.filter(|value| !value.is_empty()) {
            params.push(("pageToken", page_token));
        }
        let value = self.get_json(
            GMAIL_READONLY_SCOPE,
            &format!(
                "{}/gmail/v1/users/{}/threads",
                self.gmail_base,
                url_component(&ctx.account)
            ),
            &params,
        )?;
        Ok(GmailSearchPage {
            thread_ids: value
                .get("threads")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|thread| thread.get("id").and_then(Value::as_str))
                .filter(|id| !id.trim().is_empty())
                .map(str::to_string)
                .collect(),
            next_page_token: value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn fetch_thread(
        &self,
        ctx: &super::types::ConnectorCtx,
        thread_id: &str,
    ) -> Result<super::email::FetchedThread> {
        let value = self.get_json(
            GMAIL_READONLY_SCOPE,
            &format!(
                "{}/gmail/v1/users/{}/threads/{}",
                self.gmail_base,
                url_component(&ctx.account),
                url_component(thread_id)
            ),
            &[("format", "full")],
        )?;
        let normalized = normalize_gmail_thread(value);
        parse_gmail_thread_from_value(normalized, thread_id)
    }
}

impl CalendarTransport for NativeGoogleClient {
    fn fetch(&self, request: &CalendarFetchRequest) -> Result<CalendarFetchBatch> {
        let calendars = if request.calendars.is_empty() {
            self.calendar_ids()?
        } else {
            request.calendars.clone()
        };
        let current_calendar_ids = calendars.iter().cloned().collect::<BTreeSet<_>>();
        let previous_calendar_ids = request.sync_tokens.keys().cloned().collect::<BTreeSet<_>>();
        let mut force_snapshot =
            !previous_calendar_ids.is_empty() && previous_calendar_ids != current_calendar_ids;
        loop {
            let mut events = Vec::new();
            let mut sync_tokens = BTreeMap::new();
            let effective_request;
            let request_ref = if force_snapshot {
                effective_request = CalendarFetchRequest {
                    account: request.account.clone(),
                    calendars: request.calendars.clone(),
                    scope: request.scope,
                    updated_since: None,
                    sync_tokens: BTreeMap::new(),
                };
                &effective_request
            } else {
                request
            };
            let mut retry_as_snapshot = false;
            for calendar_id in &calendars {
                let fetched = self.calendar_events_for(calendar_id, request_ref);
                let (mut page_events, sync_token) = match fetched {
                    Ok(value) => value,
                    Err(error)
                        if !force_snapshot
                            && error
                                .downcast_ref::<GoogleNativeError>()
                                .is_some_and(|error| {
                                    matches!(error, GoogleNativeError::Http { status: 410, .. })
                                }) =>
                    {
                        retry_as_snapshot = true;
                        break;
                    }
                    Err(error) => return Err(error),
                };
                for event in &mut page_events {
                    if event.calendar_id.is_none() {
                        event.calendar_id = Some(calendar_id.clone());
                    }
                }
                events.extend(page_events);
                if let Some(sync_token) = sync_token {
                    sync_tokens.insert(calendar_id.clone(), sync_token);
                }
            }
            if retry_as_snapshot {
                force_snapshot = true;
                continue;
            }
            return Ok(CalendarFetchBatch {
                events,
                next_cursor: json!({
                    "updated_since": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                    "sync_tokens": sync_tokens,
                }),
                complete_snapshot: force_snapshot,
            });
        }
    }

    fn auth_state(&self, account: &str) -> Result<CalendarAuthState> {
        match self.verify_connection(Some(account), GoogleCredentialBackendKind::OsKeyring) {
            Ok(_) => Ok(CalendarAuthState::Ready),
            Err(error) => Ok(CalendarAuthState::NeedsAuth(redact_secret_text(&format!(
                "{error:#}"
            )))),
        }
    }
}

impl NativeGoogleClient {
    fn calendar_ids(&self) -> Result<Vec<String>> {
        let mut page_token: Option<String> = None;
        let mut ids = Vec::new();
        loop {
            let mut params = vec![("minAccessRole", "reader")];
            if let Some(token) = page_token.as_deref() {
                params.push(("pageToken", token));
            }
            let value = self.get_json(
                CALENDAR_READONLY_SCOPE,
                &format!("{}/v3/users/me/calendarList", self.calendar_base),
                &params,
            )?;
            ids.extend(
                value
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string),
            );
            page_token = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        if ids.is_empty() {
            ids.push("primary".to_string());
        }
        Ok(ids)
    }

    fn calendar_events_for(
        &self,
        calendar_id: &str,
        request: &CalendarFetchRequest,
    ) -> Result<(Vec<CalendarEvent>, Option<String>)> {
        let mut page_token: Option<String> = None;
        let mut events = Vec::new();
        let mut next_sync_token = None;
        loop {
            let mut owned = Vec::<(String, String)>::new();
            owned.push(("maxResults".into(), "2500".into()));
            owned.push(("showDeleted".into(), "true".into()));
            if let Some(sync_token) = request.sync_tokens.get(calendar_id) {
                owned.push(("syncToken".into(), sync_token.clone()));
            } else if let Some(updated_since) = request.updated_since {
                owned.push((
                    "updatedMin".into(),
                    updated_since.to_rfc3339_opts(SecondsFormat::Secs, true),
                ));
            } else {
                owned.push((
                    "timeMin".into(),
                    request
                        .scope
                        .occurred_from
                        .to_rfc3339_opts(SecondsFormat::Secs, true),
                ));
                owned.push((
                    "timeMax".into(),
                    request
                        .scope
                        .occurred_to
                        .to_rfc3339_opts(SecondsFormat::Secs, true),
                ));
                owned.push(("singleEvents".into(), "true".into()));
            }
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".into(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                CALENDAR_READONLY_SCOPE,
                &format!(
                    "{}/v3/calendars/{}/events",
                    self.calendar_base,
                    url_component(calendar_id)
                ),
                &refs,
            )?;
            let mut page = value
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|event| {
                    serde_json::from_value::<CalendarEvent>(event)
                        .context("failed to decode Calendar event")
                })
                .collect::<Result<Vec<_>>>()?;
            for event in &mut page {
                if event.calendar_id.is_none() {
                    event.calendar_id = Some(calendar_id.to_string());
                }
            }
            events.extend(page);
            next_sync_token = value
                .get("nextSyncToken")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(next_sync_token);
            page_token = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        Ok((events, next_sync_token))
    }
}

impl GoogleMeetTransport for NativeGoogleClient {
    fn fetch(&self, _account: &str, cursor: Option<&Value>) -> Result<GoogleMeetSnapshot> {
        let folder_id = self.meet_recordings_folder()?;
        let documents = if let Some(folder_id) = folder_id.as_deref() {
            self.drive_transcript_documents(
                folder_id,
                cursor
                    .and_then(|value| value.get("modified_time"))
                    .and_then(Value::as_str),
            )?
        } else {
            Vec::new()
        };
        let conferences = self.meet_conferences()?;
        let next_cursor = documents
            .iter()
            .filter_map(|document| document.modified_time.or(Some(document.created_time)))
            .max()
            .map(|modified_time| json!({"modified_time": modified_time.to_rfc3339()}))
            .or_else(|| cursor.cloned());
        Ok(GoogleMeetSnapshot {
            meet_recordings_folder_id: folder_id.unwrap_or_default(),
            documents,
            conferences,
            next_cursor,
        })
    }

    fn health(&self, account: &str) -> Result<HealthStatus> {
        self.verify_connection(Some(account), GoogleCredentialBackendKind::OsKeyring)?;
        Ok(HealthStatus::Fresh)
    }
}

impl NativeGoogleClient {
    fn meet_recordings_folder(&self) -> Result<Option<String>> {
        let mut page_token: Option<String> = None;
        let mut seen_tokens = BTreeSet::new();
        let mut matches = Vec::new();
        loop {
            let mut owned = vec![
                (
                    "q".to_string(),
                    "name = 'Meet Recordings' and mimeType = 'application/vnd.google-apps.folder' and trashed = false".to_string(),
                ),
                ("pageSize".to_string(), "10".to_string()),
                (
                    "fields".to_string(),
                    "files(id,name),nextPageToken".to_string(),
                ),
            ];
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".to_string(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                DRIVE_READONLY_SCOPE,
                &format!("{}/v3/files", self.drive_base),
                &refs,
            )?;
            matches.extend(
                value
                    .get("files")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|item| {
                        item.get("name").and_then(Value::as_str) == Some("Meet Recordings")
                    })
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_string),
            );
            let next = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_string);
            let Some(next) = next else { break };
            if !seen_tokens.insert(next.clone()) {
                bail!("Drive Meet Recordings folder discovery repeated page token {next}");
            }
            page_token = Some(next);
        }
        matches.sort();
        matches.dedup();
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.into_iter().next()),
            count => Err(GoogleNativeError::Http {
                status: 409,
                message: format!("found {count} matching Meet Recordings folders"),
            }
            .into()),
        }
    }

    fn drive_transcript_documents(
        &self,
        folder_id: &str,
        modified_after: Option<&str>,
    ) -> Result<Vec<DriveTranscriptDocument>> {
        let mut query =
            "mimeType = 'application/vnd.google-apps.document' and trashed = false".to_string();
        if let Some(modified_after) = modified_after {
            query.push_str(" and modifiedTime > '");
            query.push_str(modified_after);
            query.push('\'');
        }
        let mut page_token: Option<String> = None;
        let mut documents = Vec::new();
        loop {
            let mut owned = vec![
                (
                    "q".to_string(),
                    format!("'{folder_id}' in parents and {query}"),
                ),
                ("pageSize".to_string(), "1000".to_string()),
                (
                    "fields".to_string(),
                    "files(id,name,createdTime,modifiedTime,webViewLink),nextPageToken".to_string(),
                ),
            ];
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".to_string(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                DRIVE_READONLY_SCOPE,
                &format!("{}/v3/files", self.drive_base),
                &refs,
            )?;
            for item in value
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .context("Drive document has no id")?
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(&id)
                    .to_string();
                let created_time = json_time(item, "createdTime")
                    .with_context(|| format!("Drive document {id} has no created time"))?;
                let modified_time = json_time(item, "modifiedTime");
                let body = self.docs_text(&id)?;
                documents.push(DriveTranscriptDocument {
                    id,
                    name,
                    created_time,
                    modified_time,
                    body,
                    web_view_link: item
                        .get("webViewLink")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    raw: item.clone(),
                });
            }
            page_token = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        Ok(documents)
    }

    fn docs_text(&self, document_id: &str) -> Result<String> {
        let value = self.get_json(
            DOCS_READONLY_SCOPE,
            &format!(
                "{}/v1/documents/{}",
                self.docs_base,
                url_component(document_id)
            ),
            &[("includeTabsContent", "true")],
        )?;
        let mut out = String::new();
        collect_docs_text(&value, &mut out);
        Ok(out)
    }

    fn meet_conferences(&self) -> Result<Vec<MeetConferenceRecord>> {
        let mut page_token: Option<String> = None;
        let mut conferences = Vec::new();
        loop {
            let mut owned = vec![("pageSize".to_string(), "100".to_string())];
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".to_string(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                MEET_READONLY_SCOPE,
                &format!("{}/v2/conferenceRecords", self.meet_base),
                &refs,
            )?;
            for record in value
                .get("conferenceRecords")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = record
                    .get("name")
                    .and_then(Value::as_str)
                    .context("Meet conference record has no name")?
                    .to_string();
                let start_time = json_time(record, "startTime")
                    .with_context(|| format!("Meet conference {name} has no startTime"))?;
                let end_time = json_time(record, "endTime");
                let transcripts = self.meet_transcripts(&name)?;
                let participants = self.meet_participants(&name)?;
                conferences.push(MeetConferenceRecord {
                    name,
                    start_time,
                    end_time,
                    transcripts,
                    participants,
                    calendar_attendees: Vec::<CalendarAttendeeEvidence>::new(),
                    raw: record.clone(),
                });
            }
            page_token = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if page_token.is_none() {
                break;
            }
        }
        let meet_codes = self.meet_codes_for_conferences(&conferences)?;
        let wanted_codes = meet_codes.values().cloned().collect::<BTreeSet<_>>();
        let calendar_attendees =
            self.calendar_attendees_for_meet_codes(&conferences, &wanted_codes)?;
        for conference in &mut conferences {
            if let Some(code) = meet_codes.get(&conference.name) {
                conference.calendar_attendees = calendar_attendees
                    .get(code.as_str())
                    .cloned()
                    .unwrap_or_default();
            }
        }
        Ok(conferences)
    }

    fn meet_codes_for_conferences(
        &self,
        conferences: &[MeetConferenceRecord],
    ) -> Result<BTreeMap<String, String>> {
        let mut by_conference = BTreeMap::new();
        for conference in conferences {
            if let Some(code) = meet_code_embedded_in_conference(conference) {
                by_conference.insert(conference.name.clone(), code);
                continue;
            }
            let Some(space_name) = conference.raw.get("space").and_then(Value::as_str) else {
                continue;
            };
            if let Some(code) = self.meet_space_meeting_code(space_name)? {
                by_conference.insert(conference.name.clone(), code);
            }
        }
        Ok(by_conference)
    }

    fn meet_space_meeting_code(&self, space_name: &str) -> Result<Option<String>> {
        let value = match self.get_json(
            MEET_READONLY_SCOPE,
            &format!("{}/v2/{}", self.meet_base, space_name),
            &[("fields", "name,meetingCode,meetingUri")],
        ) {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<GoogleNativeError>()
                    .is_some_and(|error| {
                        matches!(error, GoogleNativeError::Http { status: 404, .. })
                    }) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok(value
            .get("meetingCode")
            .and_then(Value::as_str)
            .and_then(normalize_meet_code)
            .or_else(|| {
                value
                    .get("meetingUri")
                    .and_then(Value::as_str)
                    .and_then(meet_code_from_url)
            }))
    }

    fn calendar_attendees_for_meet_codes(
        &self,
        conferences: &[MeetConferenceRecord],
        wanted_codes: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, Vec<CalendarAttendeeEvidence>>> {
        if wanted_codes.is_empty() || conferences.is_empty() {
            return Ok(BTreeMap::new());
        }
        let min_start = conferences
            .iter()
            .map(|conference| conference.start_time)
            .min()
            .context("Meet conferences had no start times")?
            - chrono::Duration::days(1);
        let max_end = conferences
            .iter()
            .map(|conference| conference.end_time.unwrap_or(conference.start_time))
            .max()
            .context("Meet conferences had no end bounds")?
            + chrono::Duration::days(1);
        let mut by_code = BTreeMap::<String, Vec<Vec<CalendarAttendeeEvidence>>>::new();
        for calendar_id in self.calendar_ids()? {
            let mut page_token: Option<String> = None;
            let mut seen_tokens = BTreeSet::new();
            loop {
                let mut owned = vec![
                    ("maxResults".to_string(), "2500".to_string()),
                    ("showDeleted".to_string(), "false".to_string()),
                    ("singleEvents".to_string(), "true".to_string()),
                    (
                        "timeMin".to_string(),
                        min_start.to_rfc3339_opts(SecondsFormat::Secs, true),
                    ),
                    (
                        "timeMax".to_string(),
                        max_end.to_rfc3339_opts(SecondsFormat::Secs, true),
                    ),
                    (
                        "fields".to_string(),
                        "items(id,htmlLink,hangoutLink,conferenceData(conferenceId,entryPoints(entryPointType,uri)),attendees(displayName,email,id)),nextPageToken".to_string(),
                    ),
                ];
                if let Some(page_token) = page_token.as_deref() {
                    owned.push(("pageToken".to_string(), page_token.to_string()));
                }
                let refs = owned
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect::<Vec<_>>();
                let value = self.get_json(
                    CALENDAR_READONLY_SCOPE,
                    &format!(
                        "{}/v3/calendars/{}/events",
                        self.calendar_base,
                        url_component(&calendar_id)
                    ),
                    &refs,
                )?;
                for item in value
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let Some(code) = meet_code_for_calendar_event(item) else {
                        continue;
                    };
                    if !wanted_codes.contains(&code) {
                        continue;
                    }
                    let attendees = calendar_attendees_for_event(item);
                    if !attendees.is_empty() {
                        by_code.entry(code).or_default().push(attendees);
                    }
                }
                let next = value
                    .get("nextPageToken")
                    .and_then(Value::as_str)
                    .filter(|token| !token.is_empty())
                    .map(str::to_string);
                let Some(next) = next else { break };
                if !seen_tokens.insert(next.clone()) {
                    bail!("Calendar Meet attendee correlation repeated page token {next}");
                }
                page_token = Some(next);
            }
        }
        Ok(by_code
            .into_iter()
            .filter_map(|(code, matches)| {
                if matches.len() == 1 {
                    Some((code, matches.into_iter().next().unwrap()))
                } else {
                    None
                }
            })
            .collect())
    }

    fn meet_transcripts(&self, conference_name: &str) -> Result<Vec<MeetTranscriptMetadata>> {
        let mut page_token: Option<String> = None;
        let mut seen_tokens = BTreeSet::new();
        let mut transcripts = Vec::new();
        loop {
            let mut owned = vec![("pageSize".to_string(), "100".to_string())];
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".to_string(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                MEET_READONLY_SCOPE,
                &format!("{}/v2/{}/transcripts", self.meet_base, conference_name),
                &refs,
            )?;
            for item in value
                .get("transcripts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                transcripts.push(MeetTranscriptMetadata {
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .context("Meet transcript has no name")?
                        .to_string(),
                    document: item
                        .pointer("/docsDestination/document")
                        .and_then(Value::as_str)
                        .context("Meet transcript has no Docs destination")?
                        .to_string(),
                    start_time: json_time(item, "startTime")
                        .context("Meet transcript has no startTime")?,
                    end_time: json_time(item, "endTime"),
                });
            }
            let next = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_string);
            let Some(next) = next else { break };
            if !seen_tokens.insert(next.clone()) {
                bail!("Meet transcripts repeated page token {next}");
            }
            page_token = Some(next);
        }
        Ok(transcripts)
    }

    fn meet_participants(&self, conference_name: &str) -> Result<Vec<MeetParticipantEvidence>> {
        let mut page_token: Option<String> = None;
        let mut seen_tokens = BTreeSet::new();
        let mut participants = Vec::new();
        loop {
            let mut owned = vec![("pageSize".to_string(), "100".to_string())];
            if let Some(page_token) = page_token.as_deref() {
                owned.push(("pageToken".to_string(), page_token.to_string()));
            }
            let refs = owned
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect::<Vec<_>>();
            let value = self.get_json(
                MEET_READONLY_SCOPE,
                &format!("{}/v2/{}/participants", self.meet_base, conference_name),
                &refs,
            )?;
            for item in value
                .get("participants")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let user = item
                    .get("signedinUser")
                    .or_else(|| item.get("anonymousUser"))
                    .or_else(|| item.get("phoneUser"));
                if let Some(user) = user {
                    participants.push(MeetParticipantEvidence {
                        resource_name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown-participant")
                            .to_string(),
                        display_name: user
                            .get("displayName")
                            .and_then(Value::as_str)
                            .unwrap_or("Unknown attendee")
                            .to_string(),
                        google_user_id: user
                            .get("user")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    });
                }
            }
            let next = value
                .get("nextPageToken")
                .and_then(Value::as_str)
                .filter(|token| !token.is_empty())
                .map(str::to_string);
            let Some(next) = next else { break };
            if !seen_tokens.insert(next.clone()) {
                bail!("Meet participants repeated page token {next}");
            }
            page_token = Some(next);
        }
        Ok(participants)
    }
}

fn normalize_gmail_thread(value: Value) -> Value {
    let mut messages = Vec::new();
    for message in value
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let headers = message
            .pointer("/payload/headers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|header| {
                let name = header.get("name").and_then(Value::as_str)?;
                let value = header.get("value").and_then(Value::as_str)?;
                Some((
                    name.to_ascii_lowercase().replace('-', "_"),
                    Value::String(value.to_string()),
                ))
            })
            .collect::<serde_json::Map<_, _>>();
        messages.push(json!({
            "id": message.get("id").and_then(Value::as_str).unwrap_or_default(),
            "threadId": message.get("threadId").and_then(Value::as_str).unwrap_or_default(),
            "internalDate": message.get("internalDate").cloned().unwrap_or(Value::Null),
            "headers": headers,
            "body": gmail_message_text(message),
        }));
    }
    json!({
        "thread": {
            "id": value.get("id").and_then(Value::as_str).unwrap_or_default(),
            "historyId": value.get("historyId").and_then(Value::as_str),
            "messages": messages,
        }
    })
}

fn gmail_message_text(message: &Value) -> String {
    let mut out = String::new();
    let payload = message.get("payload").unwrap_or(&Value::Null);
    collect_gmail_part_text(payload, "text/plain", &mut out);
    if out.trim().is_empty() {
        collect_gmail_part_text(payload, "text/html", &mut out);
    }
    out
}

fn collect_gmail_part_text(part: &Value, mime_type: &str, out: &mut String) {
    if part.get("mimeType").and_then(Value::as_str) == Some(mime_type) {
        if let Some(data) = part.pointer("/body/data").and_then(Value::as_str) {
            if let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(data) {
                let text = String::from_utf8_lossy(&bytes);
                if mime_type == "text/html" {
                    out.push_str(&html_to_plaintext(&text));
                } else {
                    out.push_str(&text);
                }
                out.push('\n');
            }
        }
    }
    if let Some(parts) = part.get("parts").and_then(Value::as_array) {
        for child in parts {
            collect_gmail_part_text(child, mime_type, out);
        }
    }
}

fn html_to_plaintext(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut entity = String::new();
    let mut in_entity = false;
    for ch in html.chars() {
        if in_entity {
            if ch == ';' {
                out.push_str(match entity.as_str() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "nbsp" => " ",
                    "quot" => "\"",
                    "#39" => "'",
                    _ => "",
                });
                entity.clear();
                in_entity = false;
            } else if entity.len() < 12 {
                entity.push(ch);
            } else {
                in_entity = false;
                entity.clear();
            }
            continue;
        }
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            '&' if !in_tag => in_entity = true,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collect_docs_text(value: &Value, out: &mut String) {
    if let Some(text) = value
        .get("textRun")
        .and_then(|text| text.get("content"))
        .and_then(Value::as_str)
    {
        out.push_str(text);
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_docs_text(item, out);
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                if key == "textRun" {
                    continue;
                }
                collect_docs_text(child, out);
            }
        }
        _ => {}
    }
}

fn meet_code_embedded_in_conference(conference: &MeetConferenceRecord) -> Option<String> {
    conference
        .raw
        .pointer("/space/meetingCode")
        .and_then(Value::as_str)
        .and_then(normalize_meet_code)
        .or_else(|| {
            conference
                .raw
                .pointer("/space/meetingUri")
                .and_then(Value::as_str)
                .and_then(meet_code_from_url)
        })
}

fn meet_code_for_calendar_event(event: &Value) -> Option<String> {
    event
        .pointer("/conferenceData/conferenceId")
        .and_then(Value::as_str)
        .and_then(normalize_meet_code)
        .or_else(|| {
            event
                .get("hangoutLink")
                .and_then(Value::as_str)
                .and_then(meet_code_from_url)
        })
        .or_else(|| {
            event
                .pointer("/conferenceData/entryPoints")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|entry| {
                    entry.get("entryPointType").and_then(Value::as_str) == Some("video")
                })
                .find_map(|entry| {
                    entry
                        .get("uri")
                        .and_then(Value::as_str)
                        .and_then(meet_code_from_url)
                })
        })
}

fn calendar_attendees_for_event(event: &Value) -> Vec<CalendarAttendeeEvidence> {
    event
        .get("attendees")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attendee| {
            let email = attendee.get("email").and_then(Value::as_str)?.trim();
            if email.is_empty() {
                return None;
            }
            let display_name = attendee
                .get("displayName")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(email);
            Some(CalendarAttendeeEvidence {
                display_name: display_name.to_string(),
                email: email.to_ascii_lowercase(),
                google_user_id: attendee
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
                evidence: event
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| format!("calendar_event:{id}"))
                    .unwrap_or_else(|| "calendar_event".to_string()),
            })
        })
        .collect()
}

fn meet_code_from_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if url.host_str() != Some("meet.google.com") {
        return None;
    }
    url.path_segments()?.find_map(normalize_meet_code)
}

fn normalize_meet_code(value: &str) -> Option<String> {
    let last_segment = value.rsplit('/').next().unwrap_or(value);
    let normalized = last_segment
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn json_time(value: &Value, key: &str) -> Option<DateTime<Utc>> {
    value.get(key).and_then(Value::as_str).and_then(|text| {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|time| time.with_timezone(&Utc))
    })
}

fn url_component(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            _ => format!("%{byte:02X}").chars().collect(),
        })
        .collect()
}

pub fn redact_secret_text(input: &str) -> String {
    let mut text = input.to_string();
    for marker in [
        "access_token=",
        "refresh_token=",
        "code=",
        "auth_url=",
        "redirect_uri=",
    ] {
        let mut search_from = 0;
        while let Some(offset) = text[search_from..].to_ascii_lowercase().find(marker) {
            let start = search_from + offset;
            let value_start = start + marker.len();
            let value_end = text[value_start..]
                .find(|ch: char| ch.is_whitespace() || ch == '&' || ch == '"' || ch == '\'')
                .map(|offset| value_start + offset)
                .unwrap_or(text.len());
            text.replace_range(value_start..value_end, "[REDACTED]");
            search_from = value_start + "[REDACTED]".len();
            if search_from >= text.len() {
                break;
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::super::google_calendar::GoogleCalendarScope;
    use super::super::google_meet::GoogleMeetConnector;
    use super::super::types::ConnectorCtx;
    use super::super::{Connector, IntegrationsStore, GOOGLE_MEET_CONNECTOR_ID};
    use super::*;
    use base64::Engine;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;

    static OAUTH_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Clone)]
    struct RecordedResponse {
        status: u16,
        body: String,
        retry_after: Option<&'static str>,
    }

    impl RecordedResponse {
        fn ok(body: impl Into<String>) -> Self {
            Self {
                status: 200,
                body: body.into(),
                retry_after: None,
            }
        }

        fn status(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                retry_after: None,
            }
        }

        fn retry_after(mut self, value: &'static str) -> Self {
            self.retry_after = Some(value);
            self
        }
    }

    struct RecordedServer {
        base_url: String,
        requests: Arc<Mutex<Vec<String>>>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl RecordedServer {
        fn new(responses: Vec<RecordedResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            let handle = thread::spawn(move || {
                for response in responses {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = [0_u8; 8192];
                    let read = stream.read(&mut request).unwrap();
                    let first_line = std::str::from_utf8(&request[..read])
                        .ok()
                        .and_then(|request| request.lines().next())
                        .unwrap_or_default()
                        .to_string();
                    seen.lock().unwrap().push(first_line);
                    let reason = if response.status == 200 {
                        "OK"
                    } else {
                        "Error"
                    };
                    let mut headers = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                        response.status,
                        reason,
                        response.body.len()
                    );
                    if let Some(retry_after) = response.retry_after {
                        headers.push_str(&format!("Retry-After: {retry_after}\r\n"));
                    }
                    headers.push_str("\r\n");
                    stream.write_all(headers.as_bytes()).unwrap();
                    stream.write_all(response.body.as_bytes()).unwrap();
                    stream.flush().unwrap();
                }
            });
            Self {
                base_url,
                requests,
                handle: Some(handle),
            }
        }

        fn client(&self) -> NativeGoogleClient {
            NativeGoogleClient::with_bases(
                None,
                Some("fixture-access-token".to_string()),
                &self.base_url,
                &self.base_url,
                &self.base_url,
                &self.base_url,
                &self.base_url,
            )
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for RecordedServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    #[test]
    fn redaction_removes_oauth_values() {
        let redacted = redact_secret_text(
            "callback?code=abc123&scope=x access_token=tok refresh_token=refresh",
        );
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("access_token=tok"));
        assert!(!redacted.contains("refresh_token=refresh"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn token_cache_key_is_scope_order_independent() {
        assert_eq!(
            token_cache_key(&["scope-b", "scope-a", "scope-a"]),
            token_cache_key(&["scope-a", "scope-b"])
        );
    }

    #[test]
    fn native_gmail_payload_normalizes_to_thread_evidence_shape() {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("Hello from Gmail API");
        let value = json!({
            "id": "thread-1",
            "historyId": "123",
            "messages": [{
                "id": "msg-1",
                "threadId": "thread-1",
                "internalDate": "1787220300000",
                "payload": {
                    "headers": [
                        {"name": "From", "value": "Alice <alice@example.com>"},
                        {"name": "To", "value": "Owner <owner@example.com>"},
                        {"name": "Subject", "value": "Native"}
                    ],
                    "mimeType": "text/plain",
                    "body": {"data": body}
                }
            }]
        });
        let parsed =
            parse_gmail_thread_from_value(normalize_gmail_thread(value), "thread-1").unwrap();
        assert_eq!(parsed.id, "thread-1");
        assert!(parsed.messages[0]
            .parsed
            .body
            .contains("Hello from Gmail API"));
        assert_eq!(
            parsed.messages[0]
                .parsed
                .headers
                .get("from")
                .map(String::as_str),
            Some("Alice <alice@example.com>")
        );
    }

    #[test]
    fn gmail_html_only_payload_preserves_plaintext_conversation() {
        let html = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode("<div>Hello&nbsp;<b>from HTML</b></div>");
        let value = json!({
            "id": "thread-html",
            "messages": [{
                "id": "msg-html",
                "threadId": "thread-html",
                "internalDate": "1787220300000",
                "payload": {
                    "mimeType": "text/html",
                    "headers": [{"name":"From","value":"Sender <sender@example.com>"}],
                    "body": {"data": html}
                }
            }]
        });
        let parsed =
            parse_gmail_thread_from_value(normalize_gmail_thread(value), "thread-html").unwrap();
        assert_eq!(parsed.messages[0].parsed.body.trim(), "Hello from HTML");
    }

    #[test]
    fn gmail_attachment_id_text_part_uses_available_html_fallback() {
        let html = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode("<p>Fallback body when text part is attachment-only</p>");
        let value = json!({
            "id": "thread-attachment",
            "messages": [{
                "id": "msg-attachment",
                "threadId": "thread-attachment",
                "internalDate": "1787220300000",
                "payload": {
                    "mimeType": "multipart/alternative",
                    "headers": [{"name":"From","value":"Sender <sender@example.com>"}],
                    "parts": [
                        {"mimeType":"text/plain","body":{"attachmentId":"att-1","size":4096}},
                        {"mimeType":"text/html","body":{"data": html}}
                    ]
                }
            }]
        });
        let parsed =
            parse_gmail_thread_from_value(normalize_gmail_thread(value), "thread-attachment")
                .unwrap();
        assert_eq!(
            parsed.messages[0].parsed.body.trim(),
            "Fallback body when text part is attachment-only"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_rejects_group_readable_token_file() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(TOKEN_FILE_NAME);
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = validate_private_file_permissions(&path).unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_storage_permission_insecure"));
    }

    #[test]
    fn scope_mismatch_is_typed() {
        let missing = missing_scopes(&[GMAIL_READONLY_SCOPE.to_string()], REQUIRED_GOOGLE_SCOPES);
        let error = GoogleNativeError::ScopeMismatch { missing };
        assert_eq!(error.code(), "google_scope_mismatch");
        assert!(error.to_string().contains(CALENDAR_READONLY_SCOPE));
    }

    #[test]
    fn file_token_storage_matches_scope_supersets_without_leaking_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let storage = MarginsTokenStorage {
            account: "owner@example.com".into(),
            account_dir: temp.path().join("google/owner@example.com"),
            backend: GoogleCredentialBackendKind::File0600,
        };
        storage
            .store_token_for(
                &[GMAIL_READONLY_SCOPE, CALENDAR_READONLY_SCOPE],
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();

        let token = storage.cached_token_for(&[GMAIL_READONLY_SCOPE]).unwrap();
        assert_eq!(
            token.map(|token| token.access_token),
            Some("secret-access".into())
        );
        assert!(storage
            .cached_token_for(&[DRIVE_READONLY_SCOPE])
            .unwrap()
            .is_none());
        let stored = std::fs::read_to_string(storage.file_path()).unwrap();
        assert!(stored.contains("secret-refresh"));
        assert!(redact_secret_text("refresh_token=secret-refresh").contains("[REDACTED]"));
    }

    #[test]
    fn durable_connection_requires_metadata_token_and_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        let missing = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(missing
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_credentials_unavailable"));

        store
            .token_storage()
            .store_token_for(
                &[GMAIL_READONLY_SCOPE],
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let narrow = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(narrow
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_credentials_unavailable"));
    }

    #[test]
    fn file_backend_round_trips_across_store_instances() {
        let temp = tempfile::tempdir().unwrap();
        let first = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        first.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        first
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();

        let second = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        let metadata = second
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap();
        assert_eq!(metadata.account, "owner@example.com");
        assert_eq!(metadata.storage, GoogleCredentialBackendKind::File0600);
    }

    #[cfg(unix)]
    #[test]
    fn file_backend_restricts_google_and_account_directories() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        for path in [
            temp.path().join("google"),
            store.account_dir().to_path_buf(),
        ] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
    }

    #[test]
    fn token_provider_uses_caller_selected_storage_backend() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let provider = GoogleTokenProvider::new_with_backend(
            temp.path(),
            "owner@example.com",
            &oauth_fixture_secret(),
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        assert_eq!(
            provider.bearer_token(&[GMAIL_READONLY_SCOPE]).unwrap(),
            "secret-access"
        );
    }

    #[test]
    fn ownership_boundary_rejects_other_owner_before_oauth_token_access_or_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let desktop = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::OsKeyring,
        )
        .unwrap();
        desktop.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        let metadata_before = std::fs::read(desktop.metadata_path()).unwrap();

        let cli = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        let pre_oauth = connect_google_account(
            temp.path(),
            Some("owner@example.com"),
            GoogleCredentialBackendKind::File0600,
            b"not-an-oauth-credential",
            GoogleOAuthMode::BrowserLoopback,
            Arc::new(NoopPresenter),
        )
        .unwrap_err();
        assert_eq!(
            pre_oauth
                .downcast_ref::<GoogleNativeError>()
                .map(GoogleNativeError::code),
            Some("google_credential_backend_mismatch")
        );
        for error in [
            cli.durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
                .unwrap_err(),
            cli.forget().unwrap_err(),
        ] {
            assert_eq!(
                error
                    .downcast_ref::<GoogleNativeError>()
                    .map(GoogleNativeError::code),
                Some("google_credential_backend_mismatch")
            );
            assert!(!format!("{error:#}").contains("owner@example.com"));
        }
        assert_eq!(
            std::fs::read(desktop.metadata_path()).unwrap(),
            metadata_before
        );
        assert!(!cli.token_storage().file_path().exists());

        let staged = StagedTokenStorage::default();
        staged
            .set(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "new-access".into(),
                    refresh_token: Some("new-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let ready = GoogleConnectionReady {
            account: "owner@example.com".into(),
            storage: GoogleCredentialBackendKind::File0600,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).into())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        };
        let error = commit_verified_google_connection(&cli, &ready, &staged).unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<GoogleNativeError>()
                .map(GoogleNativeError::code),
            Some("google_credential_backend_mismatch")
        );
        assert_eq!(
            std::fs::read(desktop.metadata_path()).unwrap(),
            metadata_before
        );
        assert!(!cli.token_storage().file_path().exists());
    }

    #[test]
    fn ownership_boundary_propagates_prior_token_snapshot_errors_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        write_private_bytes(&store.token_storage().file_path(), b"not-json").unwrap();
        let metadata_before = std::fs::read(store.metadata_path()).unwrap();
        let token_before = std::fs::read(store.token_storage().file_path()).unwrap();
        let staged = StagedTokenStorage::default();
        staged
            .set(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "new-access".into(),
                    refresh_token: Some("new-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let ready = GoogleConnectionReady {
            account: "owner@example.com".into(),
            storage: GoogleCredentialBackendKind::File0600,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).into())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        };

        let error = commit_verified_google_connection(&store, &ready, &staged).unwrap_err();
        assert!(error
            .to_string()
            .contains("failed to parse Google token store"));
        assert_eq!(
            std::fs::read(store.metadata_path()).unwrap(),
            metadata_before
        );
        assert_eq!(
            std::fs::read(store.token_storage().file_path()).unwrap(),
            token_before
        );
    }

    #[test]
    fn durable_connection_rejects_corrupt_file_token() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        write_private_bytes(&store.token_storage().file_path(), b"not-json").unwrap();

        let error = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("failed to parse Google token store"));
    }

    #[cfg(unix)]
    #[test]
    fn durable_connection_rejects_insecure_file_token() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "secret-access".into(),
                    refresh_token: Some("secret-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        std::fs::set_permissions(
            store.token_storage().file_path(),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let error = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_storage_permission_insecure"));
    }

    #[test]
    fn commit_verified_connection_rolls_back_on_account_mismatch() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        let staged = StagedTokenStorage::default();
        staged
            .set(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "new-access".into(),
                    refresh_token: Some("new-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let ready = GoogleConnectionReady {
            account: "other@example.com".into(),
            storage: GoogleCredentialBackendKind::File0600,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).into())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        };
        let error = commit_verified_google_connection(&store, &ready, &staged).unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_account_mismatch"));
        assert!(!store.token_storage().file_path().exists());
        assert!(store.metadata().unwrap().is_none());
    }

    #[test]
    fn oauth_callback_validation_rejects_state_and_loopback_mismatch() {
        let expected =
            ExpectedCallback::from_redirect_uri("http://127.0.0.1:49152/oauth2/callback").unwrap();
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49152/oauth2/callback?code=ok&state=good",
            &expected,
            "good",
        )
        .is_ok());
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49152/oauth2/callback?code=ok&state=bad",
            &expected,
            "good",
        )
        .unwrap_err()
        .to_string()
        .contains("state mismatch"));
        assert!(validate_oauth_callback(
            "http://localhost:49152/oauth2/callback?code=ok&state=good",
            &expected,
            "good",
        )
        .unwrap_err()
        .to_string()
        .contains("loopback redirect"));
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49152/wrong?code=ok&state=good",
            &expected,
            "good",
        )
        .is_err());
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49153/oauth2/callback?code=ok&state=good",
            &expected,
            "good",
        )
        .is_err());
    }

    #[test]
    fn oauth_callback_validation_rejects_missing_code_and_provider_error() {
        let expected =
            ExpectedCallback::from_redirect_uri("http://127.0.0.1:49152/oauth2/callback").unwrap();
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49152/oauth2/callback?state=good",
            &expected,
            "good",
        )
        .unwrap_err()
        .to_string()
        .contains("missing code"));
        assert!(validate_oauth_callback(
            "http://127.0.0.1:49152/oauth2/callback?error=access_denied&state=good",
            &expected,
            "good",
        )
        .unwrap_err()
        .to_string()
        .contains("provider returned"));
    }

    struct FailingPresenter;

    impl GoogleOAuthPresenter for FailingPresenter {
        fn present_authorization_url(
            &self,
            _url: &str,
            _need_code: bool,
        ) -> std::result::Result<String, String> {
            Err("browser launch failed with code=secret".into())
        }
    }

    struct NoopPresenter;

    impl GoogleOAuthPresenter for NoopPresenter {
        fn present_authorization_url(
            &self,
            _url: &str,
            _need_code: bool,
        ) -> std::result::Result<String, String> {
            Ok(String::new())
        }
    }

    #[derive(Default)]
    struct CapturingPresenter {
        url: Arc<Mutex<Option<String>>>,
    }

    impl GoogleOAuthPresenter for CapturingPresenter {
        fn present_authorization_url(
            &self,
            url: &str,
            _need_code: bool,
        ) -> std::result::Result<String, String> {
            *self.url.lock().unwrap() = Some(url.to_string());
            Err("stop before exchange".into())
        }
    }

    fn oauth_fixture_secret() -> Vec<u8> {
        oauth_fixture_secret_with_token_uri("https://oauth2.googleapis.com/token")
    }

    fn oauth_fixture_secret_with_token_uri(token_uri: &str) -> Vec<u8> {
        json!({
            "installed": {
                "client_id": "fixture.apps.googleusercontent.com",
                "client_secret": "fixture-secret",
                "auth_uri": "https://accounts.google.com/o/oauth2/v2/auth",
                "token_uri": token_uri
            }
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn oauth_browser_launch_failure_is_propagated_and_redacted() {
        let _oauth_guard = OAUTH_TEST_LOCK.lock().unwrap();
        let error = run_installed_oauth(
            &oauth_fixture_secret(),
            GoogleOAuthMode::BrowserLoopback,
            Arc::new(FailingPresenter),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("browser launch failed"));
        assert!(!error.contains("code=secret"));
    }

    #[test]
    fn oauth_authorization_url_contains_pkce_state_and_exact_loopback_redirect() {
        let _oauth_guard = OAUTH_TEST_LOCK.lock().unwrap();
        let presenter = Arc::new(CapturingPresenter::default());
        let captured = Arc::clone(&presenter.url);
        let error = run_installed_oauth(
            &oauth_fixture_secret(),
            GoogleOAuthMode::BrowserLoopback,
            presenter,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("stop before exchange"));

        let url = url::Url::parse(captured.lock().unwrap().as_ref().unwrap()).unwrap();
        let params = url.query_pairs().into_owned().collect::<BTreeMap<_, _>>();
        assert_eq!(
            url.as_str().split('?').next().unwrap(),
            "https://accounts.google.com/o/oauth2/v2/auth"
        );
        assert_eq!(
            params.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(params
            .get("code_challenge")
            .is_some_and(|value| value.len() >= 43));
        assert!(params.get("state").is_some_and(|value| !value.is_empty()));
        let redirect_uri = params.get("redirect_uri").expect("redirect_uri");
        assert!(redirect_uri.starts_with("http://127.0.0.1:"));
        assert!(redirect_uri.ends_with("/oauth2/callback"));
    }

    #[test]
    fn oauth_browser_flow_times_out_without_callback() {
        let _oauth_guard = OAUTH_TEST_LOCK.lock().unwrap();
        std::env::set_var("MARGINS_GOOGLE_OAUTH_TIMEOUT_SECS", "0");
        let error = run_installed_oauth(
            &oauth_fixture_secret(),
            GoogleOAuthMode::BrowserLoopback,
            Arc::new(NoopPresenter),
        )
        .unwrap_err()
        .to_string();
        std::env::remove_var("MARGINS_GOOGLE_OAUTH_TIMEOUT_SECS");
        assert!(error.contains("timed out"));
    }

    struct LoopbackCallbackPresenter {
        response: Arc<Mutex<Option<String>>>,
    }

    impl GoogleOAuthPresenter for LoopbackCallbackPresenter {
        fn present_authorization_url(
            &self,
            url: &str,
            need_code: bool,
        ) -> std::result::Result<String, String> {
            assert!(!need_code, "browser mode must not prompt for copied input");
            let auth_url = url::Url::parse(url).unwrap();
            let params = auth_url
                .query_pairs()
                .into_owned()
                .collect::<BTreeMap<_, _>>();
            let state = params.get("state").unwrap().clone();
            let redirect_uri = params.get("redirect_uri").unwrap().clone();
            let response = Arc::clone(&self.response);
            thread::spawn(move || {
                let redirect = url::Url::parse(&redirect_uri).unwrap();
                let host = redirect.host_str().unwrap().to_string();
                let port = redirect.port_or_known_default().unwrap();
                let mut stream = TcpStream::connect((host.as_str(), port)).unwrap();
                let target = format!("{}?code=browser-code&state={state}", redirect.path());
                let request = format!(
                    "GET {target} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(request.as_bytes()).unwrap();
                let mut body = String::new();
                stream.read_to_string(&mut body).unwrap();
                *response.lock().unwrap() = Some(body);
            });
            Ok(String::new())
        }
    }

    #[test]
    fn oauth_browser_loopback_is_zero_copy_and_returns_no_store_success_page() {
        let _oauth_guard = OAUTH_TEST_LOCK.lock().unwrap();
        let token_server = RecordedServer::new(vec![RecordedResponse::ok(
            json!({
                "access_token": "fixture-access-token",
                "refresh_token": "fixture-refresh-token",
                "expires_in": 3600,
                "token_type": "Bearer"
            })
            .to_string(),
        )]);
        let response = Arc::new(Mutex::new(None));
        let token = run_installed_oauth(
            &oauth_fixture_secret_with_token_uri(&token_server.base_url),
            GoogleOAuthMode::BrowserLoopback,
            Arc::new(LoopbackCallbackPresenter {
                response: Arc::clone(&response),
            }),
        )
        .unwrap();
        assert_eq!(token.access_token, "fixture-access-token");
        assert_eq!(
            token.refresh_token.as_deref(),
            Some("fixture-refresh-token")
        );
        let page = response.lock().unwrap().clone().unwrap();
        assert!(page.contains("Cache-Control: no-store"));
        assert!(page.contains("Pragma: no-cache"));
        assert!(page.contains("Google connection complete. Return to Margins."));
        assert!(!page.contains("browser-code"));
        assert!(!page.contains("state="));
    }

    #[test]
    fn retry_after_delay_is_bounded_and_numeric() {
        assert_eq!(retry_delay(0, Some("2")), Duration::from_secs(2));
        assert_eq!(retry_delay(0, Some("120")), API_MAX_RETRY_AFTER);
        assert_eq!(retry_delay(2, Some("not-date")), Duration::from_millis(750));
    }

    #[test]
    fn verify_connection_reports_account_mismatch() {
        let server = RecordedServer::new(vec![RecordedResponse::ok(
            json!({"emailAddress":"other@example.com"}).to_string(),
        )]);
        let error = server
            .client()
            .verify_connection(
                Some("owner@example.com"),
                GoogleCredentialBackendKind::File0600,
            )
            .unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_account_mismatch"));
        assert!(server.requests()[0].contains("/gmail/v1/users/me/profile"));
    }

    #[test]
    fn verify_connection_without_expected_account_uses_actual_profile_identity() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"emailAddress":"Actual@Example.COM"}).to_string()),
            RecordedResponse::ok(json!({"items":[]}).to_string()),
            RecordedResponse::ok(json!({"files":[]}).to_string()),
            RecordedResponse::status(404, json!({"error":{"status":"NOT_FOUND"}}).to_string()),
            RecordedResponse::ok(json!({"conferenceRecords":[]}).to_string()),
        ]);
        let ready = server
            .client()
            .verify_connection(None, GoogleCredentialBackendKind::OsKeyring)
            .unwrap();
        assert_eq!(ready.account, "actual@example.com");
        assert_eq!(ready.storage, GoogleCredentialBackendKind::OsKeyring);
    }

    #[test]
    fn commit_verified_connection_persists_under_verified_actual_identity() {
        let temp = tempfile::tempdir().unwrap();
        let staged = StagedTokenStorage::default();
        staged
            .set(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "new-access".into(),
                    refresh_token: Some("new-refresh".into()),
                    expires_at: None,
                },
            )
            .unwrap();
        let ready = GoogleConnectionReady {
            account: "actual@example.com".into(),
            storage: GoogleCredentialBackendKind::File0600,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).into())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        };
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            &ready.account,
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        let committed = commit_verified_google_connection(&store, &ready, &staged).unwrap();
        assert_eq!(committed.account, "actual@example.com");
        assert!(store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .is_ok());
        assert!(!temp
            .path()
            .join("google")
            .join("owner@example.com")
            .exists());
    }

    #[test]
    fn initial_connect_without_refresh_token_fails_transactionally() {
        let temp = tempfile::tempdir().unwrap();
        let staged = StagedTokenStorage::default();
        staged
            .set(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "new-access".into(),
                    refresh_token: None,
                    expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                },
            )
            .unwrap();
        let ready = GoogleConnectionReady {
            account: "actual@example.com".into(),
            storage: GoogleCredentialBackendKind::File0600,
            scopes: REQUIRED_GOOGLE_SCOPES
                .iter()
                .map(|scope| (*scope).into())
                .collect(),
            access: vec!["gmail", "calendar", "drive", "docs", "meet"],
            read_only: true,
        };
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            &ready.account,
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        let error = commit_verified_google_connection(&store, &ready, &staged).unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_credentials_unavailable"));
        assert!(store.metadata().unwrap().is_none());
        assert!(!store.token_storage().file_path().exists());
    }

    #[test]
    fn durable_connection_rejects_expired_token_without_refresh_token() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "access".into(),
                    refresh_token: None,
                    expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
                },
            )
            .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        let error = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_credentials_unavailable"));
    }

    #[test]
    fn durable_connection_rejects_unexpired_token_without_refresh_token() {
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "access".into(),
                    refresh_token: None,
                    expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                },
            )
            .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        let error = store
            .durable_connection_metadata(REQUIRED_GOOGLE_SCOPES)
            .unwrap_err();
        assert!(error
            .downcast_ref::<GoogleNativeError>()
            .is_some_and(|error| error.code() == "google_credentials_unavailable"));
    }

    #[test]
    fn refresh_response_without_refresh_token_preserves_existing_refresh_token() {
        let token_server = RecordedServer::new(vec![RecordedResponse::ok(
            json!({
                "access_token": "new-access-token",
                "expires_in": 3600,
                "token_type": "Bearer"
            })
            .to_string(),
        )]);
        let temp = tempfile::tempdir().unwrap();
        let store = GoogleAccountStore::new_with_backend(
            temp.path(),
            "owner@example.com",
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        store
            .token_storage()
            .store_token_for(
                REQUIRED_GOOGLE_SCOPES,
                StoredOAuthToken {
                    access_token: "old-access-token".into(),
                    refresh_token: Some("old-refresh-token".into()),
                    expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
                },
            )
            .unwrap();
        store.write_metadata(REQUIRED_GOOGLE_SCOPES).unwrap();
        let provider = GoogleTokenProvider::new_with_backend(
            temp.path(),
            "owner@example.com",
            &oauth_fixture_secret_with_token_uri(&token_server.base_url),
            GoogleCredentialBackendKind::File0600,
        )
        .unwrap();
        assert_eq!(
            provider.bearer_token(REQUIRED_GOOGLE_SCOPES).unwrap(),
            "new-access-token"
        );
        let cached = store
            .token_storage()
            .cached_token_for(REQUIRED_GOOGLE_SCOPES)
            .unwrap()
            .unwrap();
        assert_eq!(cached.access_token, "new-access-token");
        assert_eq!(cached.refresh_token.as_deref(), Some("old-refresh-token"));
    }

    #[test]
    fn gmail_thread_and_history_fixtures_use_paginated_rest_shapes() {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("First message");
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(
                json!({"threads":[{"id":"t-1"}],"nextPageToken":"page-2"}).to_string(),
            ),
            RecordedResponse::ok(json!({"threads":[{"id":"t-2"}]}).to_string()),
            RecordedResponse::ok(
                json!({
                    "id": "t-1",
                    "historyId": "100",
                    "messages": [{
                        "id": "m-1",
                        "threadId": "t-1",
                        "internalDate": "1787220300000",
                        "payload": {
                            "mimeType": "text/plain",
                            "headers": [
                                {"name":"From","value":"Sender <sender@example.com>"},
                                {"name":"To","value":"Owner <owner@example.com>"}
                            ],
                            "body": {"data": body}
                        }
                    }]
                })
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({
                    "history": [{
                        "messagesDeleted": [{
                            "message": {"id": "deleted-message", "threadId": "t-deleted"}
                        }]
                    }],
                    "nextPageToken": "history-page-2"
                })
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({
                    "history": [{
                        "messagesDeleted": [{
                            "message": {"id": "deleted-message-2", "threadId": "t-deleted-2"}
                        }]
                    }],
                    "historyId": "200"
                })
                .to_string(),
            ),
        ]);
        let client = server.client();
        let ctx = super::super::types::ConnectorCtx {
            vault_root: PathBuf::from("/tmp/native-google-fixture"),
            connector_id: super::super::EMAIL_CONNECTOR_ID.into(),
            account: "owner@example.com".into(),
            command_path: None,
        };
        let first_page = client
            .search_page(&ctx, "newer_than:365d", 10, None)
            .unwrap();
        assert_eq!(first_page.thread_ids, vec!["t-1"]);
        let second_page = client
            .search_page(
                &ctx,
                "newer_than:365d",
                10,
                first_page.next_page_token.as_deref(),
            )
            .unwrap();
        assert_eq!(second_page.thread_ids, vec!["t-2"]);
        let thread = client.fetch_thread(&ctx, "t-1").unwrap();
        assert_eq!(thread.id, "t-1");
        assert!(thread.messages[0].parsed.body.contains("First message"));
        let deletions = client
            .gmail_history_deletions("owner@example.com", "99")
            .unwrap();
        assert_eq!(
            deletions.deleted_thread_ids,
            vec!["t-deleted".to_string(), "t-deleted-2".to_string()]
        );
        assert_eq!(deletions.next_history_id.as_deref(), Some("200"));
        let requests = server.requests();
        assert!(requests
            .iter()
            .any(|line| line.contains("pageToken=page-2")));
        assert!(requests
            .iter()
            .any(|line| line.contains("historyTypes=messageDeleted")));
        assert!(requests
            .iter()
            .any(|line| line.contains("pageToken=history-page-2")));
    }

    #[test]
    fn calendar_fixture_carries_sync_tokens_and_cancelled_events() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"items":[{"id":"primary"}]}).to_string()),
            RecordedResponse::ok(
                json!({
                    "items": [{
                        "id": "cancelled-event",
                        "status": "cancelled",
                        "start": {"dateTime":"2026-08-24T12:00:00Z"},
                        "updated": "2026-08-24T12:05:00Z"
                    }],
                    "nextSyncToken": "sync-1"
                })
                .to_string(),
            ),
        ]);
        let scope = GoogleCalendarScope {
            occurred_from: "2026-08-01T00:00:00Z".parse().unwrap(),
            occurred_to: "2026-09-01T00:00:00Z".parse().unwrap(),
        };
        let client = server.client();
        let batch = super::super::google_calendar::CalendarTransport::fetch(
            &client,
            &CalendarFetchRequest {
                account: "owner@example.com".into(),
                calendars: Vec::new(),
                scope,
                updated_since: None,
                sync_tokens: BTreeMap::new(),
            },
        )
        .unwrap();
        assert_eq!(batch.events[0].calendar_id.as_deref(), Some("primary"));
        assert_eq!(batch.events[0].status.as_deref(), Some("cancelled"));
        assert_eq!(
            batch
                .next_cursor
                .pointer("/sync_tokens/primary")
                .and_then(Value::as_str),
            Some("sync-1")
        );
    }

    #[test]
    fn drive_docs_and_meet_fixture_materializes_metadata() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"files":[{"id":"folder-1","name":"Meet Recordings"}]}).to_string()),
            RecordedResponse::ok(json!({"files":[{
                "id":"doc-1",
                "name":"Transcript",
                "createdTime":"2026-08-24T12:00:00Z",
                "modifiedTime":"2026-08-24T12:10:00Z",
                "webViewLink":"https://docs.google.com/document/d/doc-1"
            }]}).to_string()),
            RecordedResponse::ok(json!({"body":{"content":[{"paragraph":{"elements":[{"textRun":{"content":"Transcript body\n"}}]}}]}}).to_string()),
            RecordedResponse::ok(json!({"conferenceRecords":[{
                "name":"conferenceRecords/abc",
                "startTime":"2026-08-24T12:00:00Z",
                "endTime":"2026-08-24T12:30:00Z",
                "space":"spaces/server-generated-space"
            }]}).to_string()),
            RecordedResponse::ok(json!({"transcripts":[{
                "name":"conferenceRecords/abc/transcripts/tr-1",
                "docsDestination":{"document":"documents/doc-1"},
                "startTime":"2026-08-24T12:00:00Z",
                "endTime":"2026-08-24T12:30:00Z"
            }]}).to_string()),
            RecordedResponse::ok(json!({"participants":[{
                "name":"conferenceRecords/abc/participants/p-1",
                "signedinUser":{"displayName":"Fixture Person","user":"users/hashable"}
            }]}).to_string()),
            RecordedResponse::ok(json!({
                "name":"spaces/server-generated-space",
                "meetingCode":"meet-space"
            }).to_string()),
            RecordedResponse::ok(json!({"items":[{"id":"primary"}]}).to_string()),
            RecordedResponse::ok(json!({"items":[{
                "id":"calendar-event-1",
                "conferenceData":{"conferenceId":"meet-space"},
                "attendees":[{"displayName":"Fixture Person","email":"fixture@example.com"}]
            }]}).to_string()),
        ]);
        let client = server.client();
        let snapshot = super::super::google_meet::GoogleMeetTransport::fetch(
            &client,
            "owner@example.com",
            None,
        )
        .unwrap();
        assert_eq!(snapshot.meet_recordings_folder_id, "folder-1");
        assert_eq!(snapshot.documents.len(), 1);
        assert!(snapshot.documents[0].body.contains("Transcript body"));
        assert_eq!(snapshot.conferences.len(), 1);
        assert_eq!(snapshot.conferences[0].transcripts.len(), 1);
        assert_eq!(snapshot.conferences[0].participants.len(), 1);
        assert_eq!(snapshot.conferences[0].calendar_attendees.len(), 1);
        assert_eq!(
            snapshot.conferences[0].calendar_attendees[0].email,
            "fixture@example.com"
        );
        assert!(server
            .requests()
            .iter()
            .any(|line| line.contains("/v1/documents/doc-1")));
        assert!(server
            .requests()
            .iter()
            .any(|line| line.contains("/v2/spaces/server-generated-space")));
    }

    #[test]
    fn native_meet_attendees_materialize_role_blind_identity_and_ambiguity() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"files":[{"id":"folder-1","name":"Meet Recordings"}]}).to_string()),
            RecordedResponse::ok(json!({"files":[{
                "id":"doc-1",
                "name":"Transcript",
                "createdTime":"2026-08-24T12:00:00Z",
                "modifiedTime":"2026-08-24T12:10:00Z",
                "webViewLink":"https://docs.google.com/document/d/doc-1"
            }]}).to_string()),
            RecordedResponse::ok(json!({"body":{"content":[{"paragraph":{"elements":[{"textRun":{"content":"Transcript body\n"}}]}}]}}).to_string()),
            RecordedResponse::ok(json!({"conferenceRecords":[{
                "name":"conferenceRecords/abc",
                "startTime":"2026-08-24T12:00:00Z",
                "endTime":"2026-08-24T12:30:00Z",
                "space":"spaces/server-generated-space"
            }]}).to_string()),
            RecordedResponse::ok(json!({"transcripts":[{
                "name":"conferenceRecords/abc/transcripts/tr-1",
                "docsDestination":{"document":"documents/doc-1"},
                "startTime":"2026-08-24T12:00:00Z",
                "endTime":"2026-08-24T12:30:00Z"
            }]}).to_string()),
            RecordedResponse::ok(json!({"participants":[{
                "name":"conferenceRecords/abc/participants/p-1",
                "signedinUser":{"displayName":"Fixture Person","user":"users/hashable"}
            },{
                "name":"conferenceRecords/abc/participants/p-2",
                "anonymousUser":{"displayName":"Unmatched Guest"}
            }]}).to_string()),
            RecordedResponse::ok(json!({
                "name":"spaces/server-generated-space",
                "meetingCode":"native-meet-code"
            }).to_string()),
            RecordedResponse::ok(json!({"items":[{"id":"primary"}]}).to_string()),
            RecordedResponse::ok(json!({"items":[{
                "id":"calendar-event-1",
                "conferenceData":{"conferenceId":"native-meet-code"},
                "attendees":[{"displayName":"Fixture Person","email":"fixture@example.com"}]
            }]}).to_string()),
        ]);
        let temp = tempfile::tempdir().unwrap();
        let ctx = ConnectorCtx {
            vault_root: temp.path().to_path_buf(),
            connector_id: GOOGLE_MEET_CONNECTOR_ID.to_string(),
            account: "owner@example.com".to_string(),
            command_path: None,
        };
        let connector = GoogleMeetConnector::new(server.client());
        connector.reconcile(&ctx, None).unwrap();
        let participants = IntegrationsStore::open(temp.path())
            .unwrap()
            .external_document_participants(&ctx)
            .unwrap();
        assert!(participants.iter().any(|participant| {
            participant.source_id == "doc-1"
                && participant.display_name == "Fixture Person"
                && participant.email.as_deref() == Some("fixture@example.com")
                && !participant.ambiguous
        }));
        assert!(participants.iter().any(|participant| {
            participant.source_id == "doc-1"
                && participant.display_name == "Unmatched Guest"
                && participant.email.is_none()
                && participant.ambiguous
        }));
    }

    #[test]
    fn meet_recordings_folder_discovery_is_paginated() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"files":[],"nextPageToken":"folder-page-2"}).to_string()),
            RecordedResponse::ok(
                json!({"files":[{"id":"folder-later","name":"Meet Recordings"}]}).to_string(),
            ),
            RecordedResponse::ok(json!({"files":[]}).to_string()),
            RecordedResponse::ok(json!({"conferenceRecords":[]}).to_string()),
        ]);
        let snapshot = super::super::google_meet::GoogleMeetTransport::fetch(
            &server.client(),
            "owner@example.com",
            None,
        )
        .unwrap();
        assert_eq!(snapshot.meet_recordings_folder_id, "folder-later");
        assert!(server
            .requests()
            .iter()
            .any(|line| line.contains("pageToken=folder-page-2")));
    }

    #[test]
    fn meet_recordings_folder_discovery_errors_on_multiple_matches() {
        let server = RecordedServer::new(vec![RecordedResponse::ok(
            json!({"files":[
                {"id":"folder-a","name":"Meet Recordings"},
                {"id":"folder-b","name":"Meet Recordings"}
            ]})
            .to_string(),
        )]);
        let error = super::super::google_meet::GoogleMeetTransport::fetch(
            &server.client(),
            "owner@example.com",
            None,
        )
        .unwrap_err();
        let native = error
            .downcast_ref::<GoogleNativeError>()
            .expect("typed Google error");
        assert_eq!(native.code(), "google_http_failed");
        assert!(native
            .to_string()
            .contains("matching Meet Recordings folders"));
    }

    #[test]
    fn meet_transcripts_and_participants_are_paginated() {
        let server = RecordedServer::new(vec![
            RecordedResponse::ok(json!({"files":[]}).to_string()),
            RecordedResponse::ok(
                json!({"conferenceRecords":[{
                    "name":"conferenceRecords/abc",
                    "startTime":"2026-08-24T12:00:00Z"
                }]})
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({
                    "transcripts":[{
                        "name":"conferenceRecords/abc/transcripts/tr-1",
                        "docsDestination":{"document":"documents/doc-1"},
                        "startTime":"2026-08-24T12:00:00Z"
                    }],
                    "nextPageToken":"tr-page-2"
                })
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({"transcripts":[{
                    "name":"conferenceRecords/abc/transcripts/tr-2",
                    "docsDestination":{"document":"documents/doc-2"},
                    "startTime":"2026-08-24T12:05:00Z"
                }]})
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({
                    "participants":[{
                        "name":"conferenceRecords/abc/participants/p-1",
                        "signedinUser":{"displayName":"One","user":"users/one"}
                    }],
                    "nextPageToken":"p-page-2"
                })
                .to_string(),
            ),
            RecordedResponse::ok(
                json!({"participants":[{
                    "name":"conferenceRecords/abc/participants/p-2",
                    "anonymousUser":{"displayName":"Two"}
                }]})
                .to_string(),
            ),
        ]);
        let snapshot = super::super::google_meet::GoogleMeetTransport::fetch(
            &server.client(),
            "owner@example.com",
            None,
        )
        .unwrap();
        assert_eq!(snapshot.conferences[0].transcripts.len(), 2);
        assert_eq!(snapshot.conferences[0].participants.len(), 2);
        let requests = server.requests();
        assert!(requests
            .iter()
            .any(|line| line.contains("pageToken=tr-page-2")));
        assert!(requests
            .iter()
            .any(|line| line.contains("pageToken=p-page-2")));
    }

    #[test]
    fn docs_text_traversal_is_exact_and_does_not_duplicate_tabs() {
        let value = json!({
            "body": {"content": [{
                "paragraph": {"elements": [{"textRun": {"content": "Body one\n"}}]}
            }]},
            "tabs": [{
                "documentTab": {"body": {"content": [{
                    "paragraph": {"elements": [{"textRun": {"content": "Tab two\n"}}]}
                }]}}
            }]
        });
        let mut out = String::new();
        collect_docs_text(&value, &mut out);
        assert_eq!(out, "Body one\nTab two\n");
    }

    #[test]
    fn quota_retry_uses_retry_after_and_eventually_succeeds() {
        let server = RecordedServer::new(vec![
            RecordedResponse::status(
                429,
                json!({"error":{"status":"RESOURCE_EXHAUSTED"}}).to_string(),
            )
            .retry_after("1"),
            RecordedResponse::ok(json!({"emailAddress":"owner@example.com"}).to_string()),
            RecordedResponse::ok(json!({"items":[]}).to_string()),
            RecordedResponse::ok(json!({"files":[]}).to_string()),
            RecordedResponse::status(404, json!({"error":{"status":"NOT_FOUND"}}).to_string()),
            RecordedResponse::ok(json!({"conferenceRecords":[]}).to_string()),
        ]);
        let ready = server
            .client()
            .verify_connection(
                Some("owner@example.com"),
                GoogleCredentialBackendKind::File0600,
            )
            .unwrap();
        assert_eq!(ready.account, "owner@example.com");
        let requests = server.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|line| line.contains("/gmail/v1/users/me/profile"))
                .count(),
            2
        );
    }

    #[test]
    fn non_quota_forbidden_does_not_retry_or_mask_scope_failures() {
        let server = RecordedServer::new(vec![RecordedResponse::status(
            403,
            json!({"error":{"status":"PERMISSION_DENIED","reason":"insufficientPermissions"}})
                .to_string(),
        )]);
        let error = server
            .client()
            .verify_connection(
                Some("owner@example.com"),
                GoogleCredentialBackendKind::File0600,
            )
            .unwrap_err();
        let native = error
            .downcast_ref::<GoogleNativeError>()
            .expect("typed Google error");
        assert_eq!(native.code(), "google_http_failed");
        assert_eq!(server.requests().len(), 1);
    }
}
