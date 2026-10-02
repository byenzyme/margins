//! Google installed-app OAuth configuration for the CLI composition.
//!
//! Official builds inject the JSON through the build environment. Source
//! builds may provide their own JSON or a runtime file without changing Git.

use anyhow::{Context, Result};

pub fn load() -> Result<Option<Vec<u8>>> {
    if let Some(path) = std::env::var_os("MARGINS_GOOGLE_OAUTH_CLIENT_FILE") {
        return std::fs::read(&path)
            .with_context(|| format!("reading Google OAuth client from {}", path.to_string_lossy()))
            .map(Some);
    }
    if let Some(json) = std::env::var_os("MARGINS_GOOGLE_OAUTH_CLIENT_JSON") {
        return Ok(Some(json.to_string_lossy().into_owned().into_bytes()));
    }
    Ok(option_env!("MARGINS_GOOGLE_OAUTH_CLIENT_JSON").map(|json| json.as_bytes().to_vec()))
}
