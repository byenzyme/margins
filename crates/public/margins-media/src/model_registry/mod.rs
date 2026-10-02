//! Pinned speech assets and their process-level model locations.
//!
//! Callers select a model kind here; provider modules only load the returned
//! directory. Preparation is additive and leaves explicit environment
//! overrides untouched.

pub mod coreml;
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub mod parakeet;

use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    CoreMl,
    ParakeetOnnx,
}

/// Resolve a configured, usable model directory without downloading assets.
pub fn resolve_model(kind: ModelKind) -> Result<Option<PathBuf>> {
    match kind {
        ModelKind::CoreMl => Ok(coreml::find_installed_model()),
        ModelKind::ParakeetOnnx => Ok(resolve_parakeet_model()?.map(|(path, _)| path)),
    }
}

pub fn resolve_coreml_dir() -> Option<PathBuf> {
    coreml::find_installed_model()
}

/// Resolve an explicitly configured Parakeet directory and its decoder kind.
/// Missing configuration is distinct from an incomplete configured directory.
pub fn resolve_parakeet_model(
) -> Result<Option<(PathBuf, crate::providers::parakeet::AsrModelKind)>> {
    use crate::providers::parakeet::missing_model_files;
    let Some(path) = env_path("MARGINS_PARAKEET_MODEL_DIR") else {
        return Ok(None);
    };
    let kind = parakeet_kind_from_env()?;
    let missing = missing_model_files(&path, kind);
    anyhow::ensure!(
        missing.is_empty(),
        "Parakeet ONNX model folder {} is missing: {}",
        path.display(),
        missing.join(", ")
    );
    Ok(Some((path, kind)))
}

pub fn parakeet_kind_from_env() -> Result<crate::providers::parakeet::AsrModelKind> {
    parse_parakeet_kind(std::env::var("MARGINS_PARAKEET_MODEL_KIND").ok().as_deref())
}

pub fn parse_parakeet_kind(
    value: Option<&str>,
) -> Result<crate::providers::parakeet::AsrModelKind> {
    use crate::providers::parakeet::AsrModelKind;
    match value.unwrap_or("tdt").trim().to_ascii_lowercase().as_str() {
        "tdt" | "tdt-v2" | "v2" | "tdt-v3" | "v3" => Ok(AsrModelKind::Tdt),
        "ctc" => Ok(AsrModelKind::Ctc),
        other => {
            anyhow::bail!("Unknown MARGINS_PARAKEET_MODEL_KIND `{other}`; use `tdt` or `ctc`.")
        }
    }
}

pub fn env_path(name: &str) -> Option<PathBuf> {
    let value =
        std::env::var_os(name).filter(|value| !value.to_string_lossy().trim().is_empty())?;
    let path = PathBuf::from(value);
    if path == std::path::Path::new("~") {
        return dirs::home_dir();
    }
    if let Some(rest) = path.to_string_lossy().strip_prefix("~/") {
        return dirs::home_dir().map(|home| home.join(rest));
    }
    Some(path)
}

/// Download or verify a pinned model and its required runtime, reporting
/// progress as a message and a fraction when known.
pub fn prepare_model(
    kind: ModelKind,
    progress: &(dyn Fn(String, Option<f32>) + Send + Sync),
) -> Result<PathBuf> {
    match kind {
        ModelKind::CoreMl => coreml::download_model(|done, total| {
            progress(
                "Downloading transcription model".into(),
                (total > 0).then_some(done as f32 / total as f32),
            );
        }),
        ModelKind::ParakeetOnnx => {
            #[cfg(target_os = "linux")]
            {
                parakeet::configure_env()?;
                parakeet::prepare(progress)?;
                return env_path("MARGINS_PARAKEET_MODEL_DIR")
                    .context("Parakeet model location was not configured");
            }
            #[cfg(not(target_os = "linux"))]
            anyhow::bail!("automatic Parakeet preparation is supported on Linux")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_run_parakeet_kind_is_tdt() {
        assert_eq!(
            parse_parakeet_kind(None).unwrap(),
            crate::providers::parakeet::AsrModelKind::Tdt
        );
        assert_eq!(
            parse_parakeet_kind(Some("tdt-v3")).unwrap(),
            crate::providers::parakeet::AsrModelKind::Tdt
        );
    }
}
