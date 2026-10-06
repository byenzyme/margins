//! Setup-time ownership of Margins' machine-level hosted catalyst credentials.
//!
//! Recall lookup never calls this module. Setup/connect and the desktop's
//! Included-mode setup write the complete bundle before catalyst generation.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
#[cfg(feature = "recall")]
use std::time::Duration;

pub const BOOTSTRAP_FILE: &str = "bootstrap.json";
pub const BUNDLE_FILE: &str = "llm-config-cache.json";
pub const INCLUDED_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const INCLUDED_CATALYST_MODEL: &str = "google/gemini-3-flash-preview";
const CACHE_PROFILE_VERSION: &str = "enzyme-llm-v1";
#[cfg(feature = "recall")]
const HOSTED_CONFIG_URL: &str = "https://api.enzyme.garden/llm/free-config";
#[cfg(feature = "recall")]
const BOOTSTRAP_HEADER: &str = "X-Enzyme-Bootstrap-Id";
#[cfg(feature = "recall")]
const MAX_RATE_LIMIT_RETRIES: usize = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BootstrapIdentity {
    bootstrap_id: String,
}

/// Exact persisted shape consumed by recall-engine's setup-time resolver.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HostedCredentialBundle {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub cached_at: u64,
    pub profile: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostedProvisioningReport {
    pub base_url: String,
    pub model: String,
    pub expires_at: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostedSetupOutcome {
    Hosted(HostedProvisioningReport),
    LocalFallback { reason: String },
}

#[cfg(feature = "recall")]
#[derive(Debug, Deserialize)]
struct BrokerCredentialResponse {
    api_key: String,
    base_url: String,
    model: String,
    #[serde(default)]
    expires_at: Option<i64>,
}

#[cfg(feature = "recall")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostedBrokerFailureKind {
    RateLimited { retry_after_secs: Option<u64> },
    Network,
    AnonymousUnavailable,
    Server,
    Malformed,
}

#[cfg(feature = "recall")]
#[derive(Debug)]
struct HostedBrokerFailure {
    kind: HostedBrokerFailureKind,
    detail: String,
}

#[cfg(feature = "recall")]
impl HostedBrokerFailure {
    fn reason(&self) -> String {
        match self.kind {
            HostedBrokerFailureKind::RateLimited {
                retry_after_secs: Some(seconds),
            } => {
                let minutes = seconds.saturating_add(59) / 60;
                format!(
                    "Hosted catalysts are busy right now; try again in {} {} — local catalysts were selected meanwhile",
                    minutes.max(1),
                    if minutes.max(1) == 1 { "minute" } else { "minutes" }
                )
            }
            HostedBrokerFailureKind::RateLimited {
                retry_after_secs: None,
            } => "Hosted catalysts are busy right now; try again in a few minutes — local catalysts were selected meanwhile".to_string(),
            HostedBrokerFailureKind::Network => "Hosted catalysts could not be reached; local catalysts were selected meanwhile".to_string(),
            HostedBrokerFailureKind::AnonymousUnavailable => "Anonymous hosted catalyst setup is unavailable; local catalysts were selected meanwhile".to_string(),
            HostedBrokerFailureKind::Server => "Hosted catalyst setup is temporarily unavailable; local catalysts were selected meanwhile".to_string(),
            HostedBrokerFailureKind::Malformed => "Hosted catalyst setup returned an invalid response; local catalysts were selected meanwhile".to_string(),
        }
    }
}

pub fn margins_home() -> Result<PathBuf> {
    margins_workflows::workspace::margins_home()
}

pub fn existing_bootstrap_id(home: &Path) -> Result<Option<String>> {
    let path = home.join(BOOTSTRAP_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let identity: BootstrapIdentity =
        serde_json::from_str(&contents).with_context(|| format!("decoding {}", path.display()))?;
    let id = identity.bootstrap_id.trim();
    if id.is_empty() {
        bail!("{} contains an empty bootstrap_id", path.display());
    }
    Ok(Some(id.to_string()))
}

/// Adopt `preferred` only when no file identity exists. The file is the
/// cross-process source of truth, allowing desktop Keychain and CLI to converge.
pub fn ensure_bootstrap_id(home: &Path, preferred: Option<&str>) -> Result<String> {
    if let Some(id) = existing_bootstrap_id(home)? {
        secure_existing_file(&home.join(BOOTSTRAP_FILE))?;
        return Ok(id);
    }
    let id = preferred
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let json = serde_json::to_vec(&BootstrapIdentity {
        bootstrap_id: id.clone(),
    })?;
    atomic_write_secret(&home.join(BOOTSTRAP_FILE), &json)?;
    Ok(id)
}

pub fn install_included_bundle(
    home: &Path,
    bootstrap_id: &str,
    api_key: &str,
    expires_at: Option<i64>,
) -> Result<HostedProvisioningReport> {
    install_bundle(
        home,
        bootstrap_id,
        api_key,
        INCLUDED_BASE_URL,
        INCLUDED_CATALYST_MODEL,
        expires_at,
    )
}

pub fn ensure_included_bundle(
    home: &Path,
    bootstrap_id: &str,
    api_key: &str,
    expires_at: Option<i64>,
) -> Result<HostedProvisioningReport> {
    let path = home.join(BUNDLE_FILE);
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("included hosted credential is empty");
    }
    if let Some(bundle) = fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<HostedCredentialBundle>(&contents).ok())
        .filter(|bundle| bundle.api_key == api_key)
    {
        let bootstrap_id = ensure_bootstrap_id(home, Some(bootstrap_id))?;
        if bundle.base_url.trim().is_empty() || bundle.model.trim().is_empty() {
            return install_included_bundle(home, &bootstrap_id, api_key, expires_at);
        }
        let report = HostedProvisioningReport {
            base_url: bundle.base_url.trim().to_string(),
            model: bundle.model.trim().to_string(),
            expires_at,
        };
        let refreshed = HostedCredentialBundle {
            api_key: api_key.to_string(),
            base_url: report.base_url.clone(),
            model: report.model.clone(),
            expires_at,
            cached_at: now_epoch_secs(),
            profile: bootstrap_profile(&bootstrap_id),
        };
        atomic_write_secret(
            &path,
            &serde_json::to_vec(&refreshed).context("encoding hosted credential bundle")?,
        )?;
        set_llm_mode(home, "hosted")?;
        return Ok(report);
    }
    install_included_bundle(home, bootstrap_id, api_key, expires_at)
}

pub fn redacted_status(home: &Path) -> serde_json::Value {
    let selected = margins_workflows::catalyst::selected_status(home);
    let expiry = redacted_expiry_status(home);
    let (usable, reason) = match selected.mode {
        margins_workflows::catalyst::CatalystMode::Hosted => (true, selected.reason),
        margins_workflows::catalyst::CatalystMode::Local => local_catalyst_status(home),
        margins_workflows::catalyst::CatalystMode::None => (false, selected.reason),
    };
    serde_json::json!({
        "mode": selected.mode.as_str(),
        "usable": usable,
        "reason": reason,
        "expiry": expiry,
    })
}

#[cfg(feature = "recall-local-model")]
fn local_catalyst_status(home: &Path) -> (bool, &'static str) {
    if crate::catalyst_model_setup::is_installed_at(home) {
        (true, "local_model_ready")
    } else {
        (false, "local_model_missing")
    }
}

#[cfg(not(feature = "recall-local-model"))]
fn local_catalyst_status(_home: &Path) -> (bool, &'static str) {
    (false, "local_model_unavailable_in_build")
}

fn redacted_expiry_status(home: &Path) -> serde_json::Value {
    let settings = match margins_workflows::machine_config::engine_settings(home) {
        Ok(settings) => settings,
        Err(_) => return expiry_view("unknown", "unknown"),
    };
    if settings.generation.as_deref() != Some("hosted") {
        return expiry_view("not_applicable", "none");
    }
    let bundle = match fs::read_to_string(home.join(BUNDLE_FILE)) {
        Ok(contents) => match serde_json::from_str::<HostedCredentialBundle>(&contents) {
            Ok(bundle) => bundle,
            Err(_) => return expiry_view("unknown", "unknown"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return expiry_view("missing", "none");
        }
        Err(_) => return expiry_view("unknown", "unknown"),
    };
    match bundle.expires_at {
        None => expiry_view("not_applicable", "none"),
        Some(expires_at) => {
            let now = now_epoch_secs() as i64;
            if expires_at <= now {
                return expiry_view("expired", "expired");
            }
            let seconds = expires_at.saturating_sub(now);
            let bucket = if seconds <= 60 * 60 {
                "within_1h"
            } else if seconds <= 24 * 60 * 60 {
                "within_24h"
            } else if seconds <= 7 * 24 * 60 * 60 {
                "within_7d"
            } else {
                "later"
            };
            expiry_view("valid", bucket)
        }
    }
}

fn expiry_view(state: &str, bucket: &str) -> serde_json::Value {
    serde_json::json!({
        "state": state,
        "bucket": bucket,
    })
}

pub fn install_bundle(
    home: &Path,
    bootstrap_id: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    expires_at: Option<i64>,
) -> Result<HostedProvisioningReport> {
    let bootstrap_id = ensure_bootstrap_id(home, Some(bootstrap_id))?;
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("included hosted credential is empty");
    }
    let base_url = base_url.trim();
    let model = model.trim();
    if base_url.is_empty() || model.is_empty() {
        bail!("hosted credential bundle is missing provider metadata");
    }
    let report = HostedProvisioningReport {
        base_url: base_url.to_string(),
        model: model.to_string(),
        expires_at,
    };
    let bundle = HostedCredentialBundle {
        api_key: api_key.to_string(),
        base_url: report.base_url.clone(),
        model: report.model.clone(),
        expires_at,
        cached_at: now_epoch_secs(),
        profile: bootstrap_profile(&bootstrap_id),
    };
    atomic_write_secret(
        &home.join(BUNDLE_FILE),
        &serde_json::to_vec(&bundle).context("encoding hosted credential bundle")?,
    )?;
    set_llm_mode(home, "hosted")?;
    Ok(report)
}

pub fn invalidate_bundle_if_key(home: &Path, rejected_api_key: &str) -> Result<bool> {
    let path = home.join(BUNDLE_FILE);
    let bundle = match fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str::<HostedCredentialBundle>(&contents).ok())
    {
        Some(bundle) => bundle,
        None => return Ok(false),
    };
    if bundle.api_key != rejected_api_key {
        return Ok(false);
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

/// Select the catalyst generator in the engine settings program
/// (`configs/settings.enzyme`).
pub fn set_llm_mode(home: &Path, mode: &str) -> Result<()> {
    margins_workflows::machine_config::set_generation(home, mode)
}

pub fn cached_bundle_for_generation(home: &Path) -> Result<Option<HostedCredentialBundle>> {
    let path = home.join(BUNDLE_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let bundle: HostedCredentialBundle =
        serde_json::from_str(&contents).with_context(|| format!("decoding {}", path.display()))?;
    if bundle.api_key.trim().is_empty()
        || bundle.base_url.trim().is_empty()
        || bundle.model.trim().is_empty()
        || bundle
            .expires_at
            .is_some_and(|expires_at| expires_at <= now_epoch_secs() as i64)
    {
        return Ok(None);
    }
    Ok(Some(bundle))
}

#[cfg(feature = "recall")]
pub fn provision_hosted_from_broker(home: &Path) -> Result<HostedProvisioningReport> {
    provision_hosted_from_broker_result(home).map_err(|failure| {
        debug_hosted_failure(&failure);
        anyhow::anyhow!(failure.reason())
    })
}

#[cfg(feature = "recall")]
pub fn provision_hosted_or_local(home: &Path) -> Result<HostedSetupOutcome> {
    match provision_hosted_from_broker_result(home) {
        Ok(report) => Ok(HostedSetupOutcome::Hosted(report)),
        Err(failure) => {
            debug_hosted_failure(&failure);
            set_llm_mode(home, "local")?;
            Ok(HostedSetupOutcome::LocalFallback {
                reason: failure.reason(),
            })
        }
    }
}

#[cfg(feature = "recall")]
fn debug_hosted_failure(failure: &HostedBrokerFailure) {
    if std::env::var_os("MARGINS_RECALL_DEBUG").is_some() {
        eprintln!("hosted catalyst setup detail: {}", failure.detail);
    }
}

#[cfg(feature = "recall")]
fn provision_hosted_from_broker_result(
    home: &Path,
) -> std::result::Result<HostedProvisioningReport, HostedBrokerFailure> {
    let bootstrap_id = ensure_bootstrap_id(home, None).map_err(|error| HostedBrokerFailure {
        kind: HostedBrokerFailureKind::Malformed,
        detail: format!("preparing hosted catalyst identity: {error:#}"),
    })?;
    let url = std::env::var("ENZYME_FREE_CONFIG_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| HOSTED_CONFIG_URL.to_string());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| HostedBrokerFailure {
            kind: HostedBrokerFailureKind::Network,
            detail: format!("building hosted catalyst HTTP client: {error:#}"),
        })?;

    let mut last_retry_after_secs = None;
    for attempt in 0..=MAX_RATE_LIMIT_RETRIES {
        let response = match client
            .get(&url)
            .header(BOOTSTRAP_HEADER, &bootstrap_id)
            .send()
        {
            Ok(response) => response,
            Err(error) => {
                return Err(HostedBrokerFailure {
                    kind: HostedBrokerFailureKind::Network,
                    detail: format!("requesting hosted catalyst credentials from {url}: {error:#}"),
                });
            }
        };
        let status = response.status();
        if status.as_u16() == 429 {
            let retry_after_secs = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
            last_retry_after_secs = retry_after_secs.or(last_retry_after_secs);
            if attempt < MAX_RATE_LIMIT_RETRIES {
                let delay = retry_delay(attempt, retry_after_secs);
                std::thread::sleep(delay);
                continue;
            }
            return Err(HostedBrokerFailure {
                kind: HostedBrokerFailureKind::RateLimited {
                    retry_after_secs: last_retry_after_secs,
                },
                detail: format!(
                    "hosted catalyst broker at {url} remained rate-limited after {} attempts (Retry-After: {:?})",
                    attempt + 1,
                    retry_after_secs
                ),
            });
        }
        if matches!(status.as_u16(), 401 | 403) {
            return Err(HostedBrokerFailure {
                kind: HostedBrokerFailureKind::AnonymousUnavailable,
                detail: format!("hosted catalyst broker at {url} returned HTTP {status}"),
            });
        }
        if status.is_server_error() || !status.is_success() {
            return Err(HostedBrokerFailure {
                kind: HostedBrokerFailureKind::Server,
                detail: format!("hosted catalyst broker at {url} returned HTTP {status}"),
            });
        }
        let credentials: BrokerCredentialResponse =
            response.json().map_err(|error| HostedBrokerFailure {
                kind: HostedBrokerFailureKind::Malformed,
                detail: format!("decoding hosted catalyst response from {url}: {error:#}"),
            })?;
        if credentials.api_key.trim().is_empty()
            || credentials.base_url.trim().is_empty()
            || credentials.model.trim().is_empty()
        {
            return Err(HostedBrokerFailure {
                kind: HostedBrokerFailureKind::Malformed,
                detail: format!("hosted catalyst response from {url} omitted required fields"),
            });
        }
        return install_bundle(
            home,
            &bootstrap_id,
            &credentials.api_key,
            &credentials.base_url,
            &credentials.model,
            credentials.expires_at,
        )
        .map_err(|error| HostedBrokerFailure {
            kind: HostedBrokerFailureKind::Malformed,
            detail: format!("publishing hosted catalyst response from {url}: {error:#}"),
        });
    }
    unreachable!("bounded hosted catalyst retry loop always returns")
}

#[cfg(feature = "recall")]
fn retry_delay(attempt: usize, retry_after_secs: Option<u64>) -> Duration {
    let jitter_ms = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64)
        .wrapping_mul(1_103_515_245)
        .wrapping_add(attempt as u64 * 12_345)
        % 151;
    let base_ms = retry_after_secs
        .map(|seconds| seconds.min(60) * 1_000)
        .unwrap_or_else(|| 250 * (attempt as u64 + 1));
    Duration::from_millis(base_ms.saturating_add(jitter_ms).min(60_000))
}

fn bootstrap_profile(bootstrap_id: &str) -> String {
    format!(
        "{CACHE_PROFILE_VERSION}:bootstrap:{:x}",
        Sha256::digest(bootstrap_id.as_bytes())
    )
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn secure_existing_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("securing {}", path.display()))?;
    }
    Ok(())
}

fn atomic_write_secret(path: &Path, contents: &[u8]) -> Result<()> {
    atomic_write(path, contents, 0o600)
}

fn atomic_write(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let temporary = parent.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("publishing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "recall")]
    const HOSTED_ENV_NAMES: [&str; 7] = [
        "ENZYME_FREE_CONFIG_URL",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_MODEL",
        "OPENROUTER_API_KEY",
        "OPENROUTER_BASE_URL",
        "OPENROUTER_MODEL",
    ];

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
    #[derive(Clone)]
    struct FakeBrokerResponse {
        status: &'static str,
        retry_after: Option<&'static str>,
        body: &'static str,
    }

    #[cfg(feature = "recall")]
    fn start_fake_broker(
        responses: Vec<FakeBrokerResponse>,
    ) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
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
                let retry_after = response
                    .retry_after
                    .map(|value| format!("Retry-After: {value}\r\n"))
                    .unwrap_or_default();
                write!(
                    stream,
                    "HTTP/1.1 {}\r\n{}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    retry_after,
                    response.body.len(),
                    response.body
                )
                .unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        });
        (format!("http://{address}/llm/free-config"), server)
    }

    #[cfg(feature = "recall")]
    fn broker_case(responses: Vec<FakeBrokerResponse>) -> (HostedSetupOutcome, Vec<String>) {
        let _env_lock = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture(&HOSTED_ENV_NAMES);
        for name in &HOSTED_ENV_NAMES[1..] {
            std::env::remove_var(name);
        }
        let (url, server) = start_fake_broker(responses);
        std::env::set_var("ENZYME_FREE_CONFIG_URL", url);
        let temp = tempfile::tempdir().unwrap();
        let outcome = provision_hosted_or_local(temp.path()).unwrap();
        let requests = server.join().unwrap();
        (outcome, requests)
    }

    #[test]
    fn bootstrap_file_wins_and_included_bundle_matches_engine_shape() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let original = ensure_bootstrap_id(home, Some("file-identity")).unwrap();
        assert_eq!(original, "file-identity");
        assert_eq!(
            ensure_bootstrap_id(home, Some("desktop-identity")).unwrap(),
            original
        );

        install_included_bundle(
            home,
            "desktop-identity",
            "fixture-key-not-secret",
            Some(2_000_000_000),
        )
        .unwrap();
        let bundle: HostedCredentialBundle =
            serde_json::from_slice(&fs::read(home.join(BUNDLE_FILE)).unwrap()).unwrap();
        assert_eq!(bundle.api_key, "fixture-key-not-secret");
        assert_eq!(bundle.base_url, INCLUDED_BASE_URL);
        assert_eq!(bundle.model, INCLUDED_CATALYST_MODEL);
        assert_eq!(bundle.profile, bootstrap_profile("file-identity"));
        assert!(bundle.cached_at > 0);
        let settings = fs::read_to_string(home.join("configs/settings.enzyme")).unwrap();
        assert!(settings.contains("generation hosted"), "{settings}");
        assert!(settings.contains("updates disabled"), "{settings}");
        assert!(!home.join("config.toml").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.join(BUNDLE_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(home.join(BOOTSTRAP_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn rejected_key_only_invalidates_its_own_bundle() {
        let temp = tempfile::tempdir().unwrap();
        install_included_bundle(temp.path(), "machine", "fixture-key", None).unwrap();
        assert!(!invalidate_bundle_if_key(temp.path(), "other-key").unwrap());
        assert!(temp.path().join(BUNDLE_FILE).exists());
        assert!(invalidate_bundle_if_key(temp.path(), "fixture-key").unwrap());
        assert!(!temp.path().join(BUNDLE_FILE).exists());
    }

    #[test]
    fn cached_included_lease_refreshes_changed_expiry_without_clobbering_endpoint_or_model() {
        let temp = tempfile::tempdir().unwrap();
        install_bundle(
            temp.path(),
            "machine",
            "fixture-key",
            "https://broker-selected.invalid/v1",
            "broker-selected-model",
            Some(2_000_000_000),
        )
        .unwrap();
        let report =
            ensure_included_bundle(temp.path(), "machine", "fixture-key", Some(2_000_000_001))
                .unwrap();
        assert_eq!(report.base_url, "https://broker-selected.invalid/v1");
        assert_eq!(report.model, "broker-selected-model");
        assert_eq!(report.expires_at, Some(2_000_000_001));
        let bundle: HostedCredentialBundle =
            serde_json::from_slice(&fs::read(temp.path().join(BUNDLE_FILE)).unwrap()).unwrap();
        assert_eq!(bundle.base_url, "https://broker-selected.invalid/v1");
        assert_eq!(bundle.model, "broker-selected-model");
        assert_eq!(bundle.expires_at, Some(2_000_000_001));
    }

    #[test]
    fn cached_included_lease_refreshes_expired_same_key() {
        let temp = tempfile::tempdir().unwrap();
        install_bundle(
            temp.path(),
            "machine",
            "fixture-key",
            "https://broker-selected.invalid/v1",
            "broker-selected-model",
            Some(1),
        )
        .unwrap();

        let report =
            ensure_included_bundle(temp.path(), "machine", "fixture-key", Some(4_102_444_800))
                .unwrap();

        assert_eq!(report.base_url, "https://broker-selected.invalid/v1");
        assert_eq!(report.model, "broker-selected-model");
        assert_eq!(report.expires_at, Some(4_102_444_800));
        let bundle: HostedCredentialBundle =
            serde_json::from_slice(&fs::read(temp.path().join(BUNDLE_FILE)).unwrap()).unwrap();
        assert_eq!(bundle.api_key, "fixture-key");
        assert_eq!(bundle.expires_at, Some(4_102_444_800));
        assert!(cached_bundle_for_generation(temp.path()).unwrap().is_some());
    }

    #[test]
    fn redacted_catalyst_status_reports_expiry_bucket_without_secret_fields() {
        let _env_lock = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture(&["OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY");
        let temp = tempfile::tempdir().unwrap();
        install_bundle(
            temp.path(),
            "machine",
            "sk-or-v1-secret-shaped-fixture",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(now_epoch_secs() as i64 + 2 * 60 * 60),
        )
        .unwrap();

        let status = redacted_status(temp.path());

        assert_eq!(status["mode"], "hosted");
        assert_eq!(status["usable"], true);
        assert_eq!(status["reason"], "hosted_bundle_ready");
        assert_eq!(status["expiry"]["state"], "valid");
        assert_eq!(status["expiry"]["bucket"], "within_24h");
        let rendered = status.to_string();
        let home_path = temp.path().to_string_lossy().to_string();
        for forbidden in [
            "sk-or-v1-secret-shaped-fixture".to_string(),
            "fixture.invalid".to_string(),
            "bootstrap".to_string(),
            "profile".to_string(),
            BUNDLE_FILE.to_string(),
            home_path,
        ] {
            assert!(
                !rendered.contains(&forbidden),
                "leaked {forbidden}: {rendered}"
            );
        }
    }

    #[test]
    fn redacted_catalyst_status_does_not_call_an_uninstalled_local_model_usable() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("config.toml"), "[llm]\nmode = \"local\"\n").unwrap();

        let status = redacted_status(temp.path());

        assert_eq!(status["mode"], "local");
        assert_eq!(status["usable"], false);
        #[cfg(feature = "recall-local-model")]
        assert_eq!(status["reason"], "local_model_missing");
        #[cfg(not(feature = "recall-local-model"))]
        assert_eq!(status["reason"], "local_model_unavailable_in_build");
        assert_eq!(status["expiry"]["state"], "not_applicable");
    }

    #[cfg(feature = "recall")]
    #[test]
    fn standalone_setup_uses_shared_enzyme_broker_contract() {
        assert_eq!(
            (HOSTED_CONFIG_URL, BOOTSTRAP_HEADER),
            (
                "https://api.enzyme.garden/llm/free-config",
                "X-Enzyme-Bootstrap-Id",
            )
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn setup_fetches_fake_broker_once_and_publishes_complete_bundle() {
        use std::io::{Read, Write as _};

        let _env_lock = crate::test_process_env_lock().lock().unwrap();
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
            let body = r#"{"api_key":"fixture-hosted-key","base_url":"https://fixture.invalid/v1","model":"fixture-catalyst-model","expires_at":4102444800}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        std::env::set_var(
            "ENZYME_FREE_CONFIG_URL",
            format!("http://{address}/llm/free-config"),
        );
        let temp = tempfile::tempdir().unwrap();

        let report = provision_hosted_from_broker(temp.path()).unwrap();
        let request = server.join().unwrap();
        let bootstrap = existing_bootstrap_id(temp.path()).unwrap().unwrap();
        assert!(request.starts_with("GET /llm/free-config HTTP/1.1\r\n"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("X-Enzyme-Bootstrap-Id: {bootstrap}"))));
        assert_eq!(report.base_url, "https://fixture.invalid/v1");
        assert_eq!(report.model, "fixture-catalyst-model");
        let bundle: HostedCredentialBundle =
            serde_json::from_slice(&fs::read(temp.path().join(BUNDLE_FILE)).unwrap()).unwrap();
        assert_eq!(bundle.api_key, "fixture-hosted-key");
        assert_eq!(bundle.base_url, report.base_url);
        assert_eq!(bundle.model, report.model);
        assert_eq!(bundle.profile, bootstrap_profile(&bootstrap));
        assert!(fs::read_to_string(temp.path().join("configs/settings.enzyme"))
            .unwrap()
            .contains("generation hosted"));
    }

    #[cfg(feature = "recall")]
    #[test]
    fn setup_retries_rate_limit_and_reaches_a_valid_hosted_lease() {
        let success = r#"{"api_key":"fixture-hosted-key","base_url":"https://fixture.invalid/v1","model":"fixture-catalyst-model","expires_at":4102444800}"#;
        let (outcome, requests) = broker_case(vec![
            FakeBrokerResponse {
                status: "429 Too Many Requests",
                retry_after: Some("0"),
                body: "{}",
            },
            FakeBrokerResponse {
                status: "200 OK",
                retry_after: None,
                body: success,
            },
        ]);

        assert!(matches!(outcome, HostedSetupOutcome::Hosted(_)));
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            request.starts_with("GET /llm/free-config HTTP/1.1\r\n")
                && request.lines().any(|line| {
                    line.to_ascii_lowercase()
                        .starts_with("x-enzyme-bootstrap-id:")
                })
        }));
    }

    #[cfg(feature = "recall")]
    #[test]
    fn retry_after_advice_keeps_server_minutes_while_sleep_is_capped() {
        let failure = HostedBrokerFailure {
            kind: HostedBrokerFailureKind::RateLimited {
                retry_after_secs: Some(125),
            },
            detail: "debug only".to_string(),
        };

        assert_eq!(
            failure.reason(),
            "Hosted catalysts are busy right now; try again in 3 minutes — local catalysts were selected meanwhile"
        );
        assert!(retry_delay(0, Some(125)) <= Duration::from_secs(60));
    }

    #[cfg(feature = "recall")]
    #[test]
    fn exhausted_rate_limit_with_retry_after_falls_back_with_minutes() {
        let limited = FakeBrokerResponse {
            status: "429 Too Many Requests",
            retry_after: Some("0"),
            body: "{}",
        };
        let (outcome, requests) = broker_case(vec![limited.clone(), limited.clone(), limited]);

        assert_eq!(requests.len(), 3);
        assert_eq!(
            outcome,
            HostedSetupOutcome::LocalFallback {
                reason: "Hosted catalysts are busy right now; try again in 1 minute — local catalysts were selected meanwhile".to_string(),
            }
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn exhausted_rate_limit_without_retry_after_uses_bounded_jittered_retries() {
        let limited = FakeBrokerResponse {
            status: "429 Too Many Requests",
            retry_after: None,
            body: "{}",
        };
        let (outcome, requests) = broker_case(vec![limited.clone(), limited.clone(), limited]);

        assert_eq!(requests.len(), 3);
        assert_eq!(
            outcome,
            HostedSetupOutcome::LocalFallback {
                reason: "Hosted catalysts are busy right now; try again in a few minutes — local catalysts were selected meanwhile".to_string(),
            }
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn unauthorized_and_forbidden_are_margins_worded() {
        for status in ["401 Unauthorized", "403 Forbidden"] {
            let (outcome, requests) = broker_case(vec![FakeBrokerResponse {
                status,
                retry_after: None,
                body: "{}",
            }]);
            assert_eq!(requests.len(), 1);
            assert_eq!(
                outcome,
                HostedSetupOutcome::LocalFallback {
                    reason: "Anonymous hosted catalyst setup is unavailable; local catalysts were selected meanwhile".to_string(),
                }
            );
        }
    }

    #[cfg(feature = "recall")]
    #[test]
    fn server_and_malformed_responses_are_margins_worded() {
        let (server_outcome, _) = broker_case(vec![FakeBrokerResponse {
            status: "503 Service Unavailable",
            retry_after: None,
            body: "{}",
        }]);
        assert_eq!(
            server_outcome,
            HostedSetupOutcome::LocalFallback {
                reason: "Hosted catalyst setup is temporarily unavailable; local catalysts were selected meanwhile".to_string(),
            }
        );

        let (malformed_outcome, _) = broker_case(vec![FakeBrokerResponse {
            status: "200 OK",
            retry_after: None,
            body: "not-json",
        }]);
        assert_eq!(
            malformed_outcome,
            HostedSetupOutcome::LocalFallback {
                reason: "Hosted catalyst setup returned an invalid response; local catalysts were selected meanwhile".to_string(),
            }
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn connection_refused_is_margins_worded() {
        let _env_lock = crate::test_process_env_lock().lock().unwrap();
        let _restore = EnvRestore::capture(&HOSTED_ENV_NAMES);
        for name in &HOSTED_ENV_NAMES[1..] {
            std::env::remove_var(name);
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        std::env::set_var(
            "ENZYME_FREE_CONFIG_URL",
            format!("http://{address}/llm/free-config"),
        );
        let temp = tempfile::tempdir().unwrap();

        assert_eq!(
            provision_hosted_or_local(temp.path()).unwrap(),
            HostedSetupOutcome::LocalFallback {
                reason:
                    "Hosted catalysts could not be reached; local catalysts were selected meanwhile"
                        .to_string(),
            }
        );
    }
}
