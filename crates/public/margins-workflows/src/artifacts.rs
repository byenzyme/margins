//! Confined artifact resolution, listing, and expiry pruning.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use margins_store::legacy::{self, SessionArtifact};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactView {
    pub artifact: SessionArtifact,
    pub disk_path: PathBuf,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunedArtifact {
    pub artifact: SessionArtifact,
    pub disk_path: PathBuf,
    pub deleted: bool,
    pub registry_rows: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactPruneReport {
    pub artifacts: Vec<PrunedArtifact>,
    pub deleted: usize,
    pub registry_rows: usize,
    pub rejected_paths: Vec<String>,
}

pub fn artifact_registry_disk_path(
    work_dir: &Path,
    margins_dir: &Path,
    registry_path: &str,
) -> PathBuf {
    let path = Path::new(registry_path);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(stripped) = path.strip_prefix(".margins") {
        return margins_dir.join(stripped);
    }
    work_dir.join(path)
}

/// Resolves only `.margins/artifacts/<session>/<child...>` registry paths.
/// Absolute paths, traversal, platform prefixes, and artifact-root targets are
/// rejected before any filesystem operation.
pub fn confined_artifact_registry_disk_path(
    margins_dir: &Path,
    registry_path: &str,
) -> Option<PathBuf> {
    let path = Path::new(registry_path);
    if path.is_absolute() {
        return None;
    }
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(first)), Some(Component::Normal(second)))
            if first == OsStr::new(".margins") && second == OsStr::new("artifacts") => {}
        _ => return None,
    }
    let mut out = margins_dir.join("artifacts");
    let mut child_count = 0usize;
    for component in components {
        match component {
            Component::Normal(part) => {
                out.push(part);
                child_count += 1;
            }
            _ => return None,
        }
    }
    (child_count >= 2).then_some(out)
}

/// Resolve an artifact path only when it belongs to the registry row's own
/// session and no existing component below `artifacts/` is a symlink.
///
/// The lexical check prevents traversal and cross-session deletion. The
/// component walk prevents an in-tree symlink from redirecting reads or
/// deletes outside the artifact root.
pub fn confined_session_artifact_registry_disk_path(
    margins_dir: &Path,
    session_name: &str,
    registry_path: &str,
) -> Option<PathBuf> {
    let session_component = single_normal_component(Path::new(session_name))?;
    let path = Path::new(registry_path);
    let mut components = path.components();
    match (components.next(), components.next(), components.next()) {
        (
            Some(Component::Normal(first)),
            Some(Component::Normal(second)),
            Some(Component::Normal(session)),
        ) if first == OsStr::new(".margins")
            && second == OsStr::new("artifacts")
            && session == session_component => {}
        _ => return None,
    }
    let mut out = margins_dir.join("artifacts").join(session_component);
    let mut child_count = 0usize;
    for component in components {
        match component {
            Component::Normal(part) => {
                out.push(part);
                child_count += 1;
            }
            _ => return None,
        }
    }
    if child_count == 0 || has_existing_symlink_below(margins_dir, &out) {
        return None;
    }
    Some(out)
}

/// Resolve a session artifact for non-destructive access. In addition to the
/// modern artifact tree, this admits the exact legacy transcript sidecars and
/// the visible `_margins/<session>_aligned.md` archive location.
pub fn confined_session_artifact_access_disk_path(
    margins_dir: &Path,
    session_name: &str,
    registry_path: &str,
) -> Option<PathBuf> {
    if let Some(path) =
        confined_session_artifact_registry_disk_path(margins_dir, session_name, registry_path)
    {
        return Some(path);
    }
    single_normal_component(Path::new(session_name))?;
    if let Some(path) = confined_legacy_capture_disk_path(margins_dir, session_name, registry_path)
    {
        return Some(path);
    }
    let allowed = [
        format!(".margins/{session_name}_aligned.md"),
        format!(".margins/{session_name}_capture_context.md"),
    ];
    if allowed.iter().any(|candidate| candidate == registry_path) {
        let file = Path::new(registry_path).file_name()?;
        let path = margins_dir.join(file);
        return (!is_symlink_or_error(&path)).then_some(path);
    }
    if registry_path != format!("_margins/{session_name}_aligned.md") {
        return None;
    }
    let archive_dir = margins_dir.parent()?.join("_margins");
    if is_symlink_or_error(&archive_dir) {
        return None;
    }
    let path = archive_dir.join(format!("{session_name}_aligned.md"));
    (!is_symlink_or_error(&path)).then_some(path)
}

/// Admit only the exact root-level filenames emitted by the terminal capture
/// adapter. These predate the per-session artifact tree but are still durable,
/// registered artifacts. The session prefix and numeric segment ordinal keep
/// another session's files and arbitrary `.margins/` children out of scope.
fn confined_legacy_capture_disk_path(
    margins_dir: &Path,
    session_name: &str,
    registry_path: &str,
) -> Option<PathBuf> {
    let path = Path::new(registry_path);
    let mut components = path.components();
    let (Some(Component::Normal(root)), Some(Component::Normal(file)), None) =
        (components.next(), components.next(), components.next())
    else {
        return None;
    };
    if root != OsStr::new(".margins") {
        return None;
    }
    let file = file.to_str()?;
    let segment = file.strip_prefix(&format!("{session_name}_seg"))?;
    let ordinal = segment
        .strip_suffix(".wav")
        .or_else(|| segment.strip_suffix(".live-transcript.json"))?;
    if ordinal.is_empty() || !ordinal.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let path = margins_dir.join(file);
    (!is_symlink_or_error(&path)).then_some(path)
}

fn single_normal_component(path: &Path) -> Option<&OsStr> {
    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(value)), None) => Some(value),
        _ => None,
    }
}

fn has_existing_symlink_below(margins_dir: &Path, path: &Path) -> bool {
    let root = margins_dir.join("artifacts");
    let Ok(relative) = path.strip_prefix(&root) else {
        return true;
    };
    let mut current = root;
    if is_symlink_or_error(&current) {
        return true;
    }
    for component in relative.components() {
        current.push(component.as_os_str());
        if is_symlink_or_error(&current) {
            return true;
        }
    }
    false
}

fn is_symlink_or_error(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata.file_type().is_symlink(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

pub fn list_artifacts(
    work_dir: &Path,
    margins_dir: &Path,
    session_name: &str,
) -> Result<Vec<ArtifactView>> {
    Ok(legacy::list_session_artifacts(margins_dir, session_name)?
        .into_iter()
        .map(|artifact| {
            let disk_path = artifact_registry_disk_path(work_dir, margins_dir, &artifact.path);
            let exists = confined_session_artifact_access_disk_path(
                margins_dir,
                &artifact.session_name,
                &artifact.path,
            )
            .is_some_and(|path| path.exists());
            ArtifactView {
                artifact,
                disk_path,
                exists,
            }
        })
        .collect())
}

pub fn prune_expired_artifacts(
    margins_dir: &Path,
    before: DateTime<Local>,
) -> Result<ArtifactPruneReport> {
    let mut report = ArtifactPruneReport::default();
    for artifact in legacy::list_expired_session_artifacts(margins_dir, before)? {
        let Some(path) = confined_session_artifact_registry_disk_path(
            margins_dir,
            &artifact.session_name,
            &artifact.path,
        ) else {
            report.rejected_paths.push(artifact.path);
            continue;
        };
        // The durable ordering is intentional: a failed file deletion keeps
        // the registry row so the next prune can retry.
        let deleted = delete_path_if_present(&path)?;
        prune_empty_artifact_dirs(margins_dir, path.parent());
        let rows = legacy::delete_session_artifact_registry_row(
            margins_dir,
            &artifact.session_name,
            &artifact.kind,
            artifact.ordinal,
        )?;
        report.deleted += usize::from(deleted);
        report.registry_rows += rows;
        report.artifacts.push(PrunedArtifact {
            artifact,
            disk_path: path,
            deleted,
            registry_rows: rows,
        });
    }
    Ok(report)
}

fn delete_path_if_present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => {
            std::fs::remove_dir_all(path).with_context(|| {
                format!("failed to delete artifact directory {}", path.display())
            })?;
            Ok(true)
        }
        Ok(_) => {
            std::fs::remove_file(path)
                .with_context(|| format!("failed to delete artifact file {}", path.display()))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect artifact path {}", path.display())),
    }
}

fn prune_empty_artifact_dirs(margins_dir: &Path, start: Option<&Path>) {
    let root = margins_dir.join("artifacts");
    let Some(mut dir) = start.map(Path::to_path_buf) else {
        return;
    };
    while dir.starts_with(&root) && dir != root {
        if std::fs::remove_dir(&dir).is_err() {
            break;
        }
        let Some(parent) = dir.parent() else { break };
        dir = parent.to_path_buf();
    }
}
