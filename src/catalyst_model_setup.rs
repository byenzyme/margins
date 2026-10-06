//! Machine-level provisioning of the local catalyst model.
//!
//! This is the third downloadable asset behind `margins setup`, alongside the
//! FluidAudio transcription and Polyvoice diarization models. It is the fine-
//! tuned model that lets `margins init` build thematic bridges offline, with
//! no key. Setup is machine-level and directory-agnostic: this only touches
//! `$MARGINS_HOME/models` (the models directory of the Margins home as an
//! Enzyme home), never a vault or `~/.enzyme/models`.
//!
//! The model's name, URL, checksum, and size come from the engine's own
//! registry (`enzyme model list --json`), so a model bump in the shipped
//! `enzyme` cannot drift from what Margins downloads.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::enzyme_cli::{Engine, ModelEntry, ModelsEnvelope};

fn margins_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("MARGINS_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".margins"))
}

/// The engine's model registry for one Margins home.
fn models(home: &Path) -> Result<ModelsEnvelope> {
    Ok(Engine::for_home(home)?.models()?)
}

/// The model `--llm local` uses: the selected model, otherwise the first
/// registry model.
fn default_model(models: &ModelsEnvelope) -> Result<&ModelEntry> {
    models
        .selected
        .as_deref()
        .and_then(|selected| models.models.iter().find(|model| model.name == selected))
        .or_else(|| models.models.iter().find(|model| model.registry))
        .context("the enzyme model registry lists no catalyst model")
}

/// Whether a correctly-sized model is already installed. Read-only: the
/// engine is only asked for its model list (see [`Engine::for_inspection`]).
pub fn is_installed() -> bool {
    margins_home()
        .ok()
        .is_some_and(|home| is_installed_at(&home))
}

/// Whether the engine would run a local model for one explicit Margins home.
/// Status reporting uses this form so it never depends on ambient process
/// environment when inspecting another configured home.
pub fn is_installed_at(home: &Path) -> bool {
    Engine::for_inspection(home)
        .and_then(|engine| Ok(engine.models()?))
        .is_ok_and(|models| models.active.is_some())
}

/// Download and verify the catalyst model into `$MARGINS_HOME/models`. No-op if a
/// correctly-sized copy is already present. Emits a single progress line; curl
/// renders its own progress bar to stderr.
pub fn ensure_installed() -> Result<PathBuf> {
    let home = margins_home()?;
    let models = models(&home)?;
    let entry = default_model(&models)?;
    let target = models.models_dir.join(format!("{}.gguf", entry.name));
    if entry.installed
        && std::fs::metadata(&target).is_ok_and(|metadata| metadata.len() == entry.size_bytes)
    {
        return Ok(target);
    }
    let (Some(url), Some(sha256)) = (entry.url.as_deref(), entry.sha256.as_deref()) else {
        bail!("model {} has no registry download", entry.name);
    };
    std::fs::create_dir_all(&models.models_dir)
        .with_context(|| format!("could not create {}", models.models_dir.display()))?;

    eprintln!(
        "Downloading recall model {} (~{} MB)…",
        entry.name,
        entry.size_bytes / 1_048_576
    );
    let tmp = target.with_extension("gguf.part");
    let status = Command::new("/usr/bin/curl")
        .args(["-fL", "--progress-bar", "-o"])
        .arg(&tmp)
        .arg(url)
        .status()
        .context("could not start curl")?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("recall model download failed (curl exited {status})");
    }

    verify_sha256(&tmp, sha256).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    std::fs::rename(&tmp, &target)
        .with_context(|| format!("could not install model to {}", target.display()))?;
    Ok(target)
}

/// Verify the downloaded file's SHA-256 with the system `shasum` tool.
fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .context("could not run shasum")?;
    if !output.status.success() {
        bail!("shasum failed on downloaded model");
    }
    let digest = String::from_utf8_lossy(&output.stdout);
    let digest = digest.split_whitespace().next().unwrap_or_default();
    if !digest.eq_ignore_ascii_case(expected) {
        bail!("recall model checksum mismatch (expected {expected}, got {digest})");
    }
    Ok(())
}
