//! Machine-level provisioning of the local catalyst model (`catalyst-135m-v3`).
//!
//! This is the third downloadable asset behind `margins setup`, alongside the
//! FluidAudio transcription and Polyvoice diarization models. It is the fine-
//! tuned model that lets `margins init` build thematic bridges offline, with
//! no key. Setup is machine-level and directory-agnostic: this only touches the
//! shared model cache, never a vault.

use anyhow::{bail, Context, Result};
use recall_engine::llm::model_registry::{self, ModelEntry, DEFAULT_MODEL};
use std::path::PathBuf;
use std::process::Command;

/// Model asset metadata — the SINGLE source is the vendored engine's registry
/// (`recall_engine::llm::model_registry`). Setup reads name/url/sha256/size from
/// there rather than keeping its own copy, so a model bump can't drift the two.
fn entry() -> &'static ModelEntry {
    model_registry::lookup(DEFAULT_MODEL)
        .expect("default catalyst model must be present in the engine registry")
}

/// The on-disk location the engine loads the model from.
pub fn model_path() -> Option<PathBuf> {
    Some(
        margins_models_dir()
            .ok()?
            .join(format!("{DEFAULT_MODEL}.gguf")),
    )
}

fn margins_home() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("MARGINS_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".margins"))
}

fn margins_models_dir() -> Result<PathBuf> {
    Ok(margins_home()?.join("models"))
}

/// Whether a correctly-sized model is already installed.
pub fn is_installed() -> bool {
    margins_home()
        .ok()
        .is_some_and(|home| is_installed_at(&home))
}

/// Whether a correctly-sized model is installed under one explicit Margins
/// home. Status reporting uses this form so it never depends on ambient
/// process environment when inspecting another configured home.
pub fn is_installed_at(home: &std::path::Path) -> bool {
    std::fs::metadata(home.join("models").join(format!("{DEFAULT_MODEL}.gguf")))
        .ok()
        .map(|m| m.len() == entry().size)
        .unwrap_or(false)
}

/// Download and verify the catalyst model into the shared cache. No-op if a
/// correctly-sized copy is already present. Emits a single progress line; curl
/// renders its own progress bar to stderr.
pub fn ensure_installed() -> Result<PathBuf> {
    let target = model_path().context("could not resolve catalyst model path")?;
    if is_installed() {
        return Ok(target);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }

    eprintln!(
        "Downloading recall model {} (~{} MB)…",
        entry().name,
        entry().size / 1_048_576
    );
    let tmp = target.with_extension("gguf.part");
    let status = Command::new("/usr/bin/curl")
        .args(["-fL", "--progress-bar", "-o"])
        .arg(&tmp)
        .arg(entry().url)
        .status()
        .context("could not start curl")?;
    if !status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!("recall model download failed (curl exited {status})");
    }

    verify_sha256(&tmp).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    std::fs::rename(&tmp, &target)
        .with_context(|| format!("could not install model to {}", target.display()))?;
    Ok(target)
}

/// Verify the downloaded file's SHA-256 with the system `shasum` tool.
fn verify_sha256(path: &std::path::Path) -> Result<()> {
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
    let expected = entry().sha256;
    if !digest.eq_ignore_ascii_case(expected) {
        bail!("recall model checksum mismatch (expected {expected}, got {digest})");
    }
    Ok(())
}
