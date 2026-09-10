//! Move durable aligned Markdown between the private store and a visible vault archive.

use crate::artifacts::confined_session_artifact_access_disk_path;
use anyhow::{bail, Context, Result};
use margins_store::legacy::{self, SessionArtifactPathUpdate, SESSION_ARTIFACT_KIND_TRANSCRIPT};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

pub const ARCHIVE_DIR_NAME: &str = "_margins";
const ARCHIVE_MARKER_NAME: &str = "archive-enabled";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveReport {
    pub enabled: bool,
    pub path: PathBuf,
    pub moved: usize,
    pub transcripts: usize,
}

#[derive(Clone, Debug)]
struct PlannedMove {
    source: PathBuf,
    target: PathBuf,
}

pub fn is_enabled(work_dir: &Path) -> bool {
    archive_marker(work_dir).is_file()
}

pub fn archive_path(work_dir: &Path) -> PathBuf {
    work_dir.join(ARCHIVE_DIR_NAME)
}

pub fn aligned_output_path(work_dir: &Path, session_name: &str, local: &Path) -> PathBuf {
    if is_enabled(work_dir) {
        archive_path(work_dir).join(format!("{session_name}_aligned.md"))
    } else {
        local.to_path_buf()
    }
}

pub fn aligned_registry_path(work_dir: &Path, session_name: &str, local: &str) -> String {
    if is_enabled(work_dir) {
        format!("{ARCHIVE_DIR_NAME}/{session_name}_aligned.md")
    } else {
        local.to_string()
    }
}

pub fn status(work_dir: &Path) -> Result<ArchiveReport> {
    Ok(ArchiveReport {
        enabled: is_enabled(work_dir),
        path: archive_path(work_dir),
        moved: 0,
        transcripts: count_archived_transcripts(work_dir)?,
    })
}

pub fn set_enabled(work_dir: &Path, enabled: bool) -> Result<ArchiveReport> {
    let margins_dir = work_dir.join(".margins");
    if !margins_dir.is_dir() {
        bail!("No Margins store found at {}.", margins_dir.display());
    }
    reject_symlink(&margins_dir)?;
    let archive_dir = archive_path(work_dir);
    if archive_dir.exists() {
        reject_symlink(&archive_dir)?;
    }

    let (moves, updates) = if enabled {
        plan_enable(&margins_dir, &archive_dir)?
    } else {
        plan_disable(work_dir, &margins_dir, &archive_dir)?
    };
    preflight_targets(&moves)?;

    if enabled {
        std::fs::create_dir_all(&archive_dir)
            .with_context(|| format!("failed to create {}", archive_dir.display()))?;
    }

    let mut completed = Vec::new();
    for planned in &moves {
        if let Some(parent) = planned.target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if let Err(error) = std::fs::rename(&planned.source, &planned.target) {
            rollback_moves(&completed);
            return Err(error).with_context(|| {
                format!(
                    "failed to move {} to {}",
                    planned.source.display(),
                    planned.target.display()
                )
            });
        }
        completed.push(planned.clone());
    }

    if let Err(error) = legacy::rewrite_session_artifact_paths(&margins_dir, &updates) {
        rollback_moves(&completed);
        return Err(error).context("failed to update transcript artifact registry");
    }

    if let Err(error) = set_marker(work_dir, enabled) {
        let reverse = updates
            .iter()
            .map(|update| SessionArtifactPathUpdate {
                session_name: update.session_name.clone(),
                kind: update.kind.clone(),
                ordinal: update.ordinal,
                old_path: update.new_path.clone(),
                new_path: update.old_path.clone(),
            })
            .collect::<Vec<_>>();
        let _ = legacy::rewrite_session_artifact_paths(&margins_dir, &reverse);
        rollback_moves(&completed);
        return Err(error);
    }

    if !enabled {
        let _ = std::fs::remove_dir(&archive_dir);
    }
    Ok(ArchiveReport {
        enabled,
        path: archive_dir,
        moved: completed.len(),
        transcripts: count_archived_transcripts(work_dir)?,
    })
}

fn plan_enable(
    margins_dir: &Path,
    archive_dir: &Path,
) -> Result<(Vec<PlannedMove>, Vec<SessionArtifactPathUpdate>)> {
    let mut by_target = BTreeMap::<PathBuf, PathBuf>::new();
    let mut updates = Vec::new();
    for session in legacy::list_sessions(margins_dir)? {
        validate_session_name(&session.name)?;
        for artifact in legacy::list_session_artifacts(margins_dir, &session.name)? {
            if artifact.kind != SESSION_ARTIFACT_KIND_TRANSCRIPT
                || artifact.path.ends_with(".live-transcript.json")
                || !artifact.path.ends_with(".md")
            {
                continue;
            }
            let target = archive_dir.join(format!("{}_aligned.md", session.name));
            let new_path = format!("{ARCHIVE_DIR_NAME}/{}_aligned.md", session.name);
            if artifact.path == new_path {
                continue;
            }
            let source = confined_session_artifact_access_disk_path(
                margins_dir,
                &session.name,
                &artifact.path,
            );
            if let Some(source) = source.filter(|path| path.is_file()) {
                insert_move(&mut by_target, source, target.clone())?;
            } else if !target.is_file() {
                continue;
            }
            updates.push(SessionArtifactPathUpdate {
                session_name: session.name.clone(),
                kind: artifact.kind,
                ordinal: artifact.ordinal,
                old_path: artifact.path,
                new_path,
            });
        }
    }

    for entry in std::fs::read_dir(margins_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || !is_aligned_markdown(&path) {
            continue;
        }
        let target = archive_dir.join(entry.file_name());
        insert_move(&mut by_target, path, target)?;
    }

    let moves = by_target
        .into_iter()
        .map(|(target, source)| PlannedMove { source, target })
        .collect();
    Ok((moves, updates))
}

fn plan_disable(
    _work_dir: &Path,
    margins_dir: &Path,
    archive_dir: &Path,
) -> Result<(Vec<PlannedMove>, Vec<SessionArtifactPathUpdate>)> {
    let mut by_target = BTreeMap::<PathBuf, PathBuf>::new();
    if archive_dir.is_dir() {
        for entry in std::fs::read_dir(archive_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type()?.is_file() || !is_aligned_markdown(&path) {
                continue;
            }
            insert_move(&mut by_target, path, margins_dir.join(entry.file_name()))?;
        }
    }

    let mut updates = Vec::new();
    for session in legacy::list_sessions(margins_dir)? {
        validate_session_name(&session.name)?;
        let archived_path = format!("{ARCHIVE_DIR_NAME}/{}_aligned.md", session.name);
        for artifact in legacy::list_session_artifacts(margins_dir, &session.name)? {
            if artifact.kind == SESSION_ARTIFACT_KIND_TRANSCRIPT
                && artifact.path == archived_path
                && (archive_dir
                    .join(format!("{}_aligned.md", session.name))
                    .is_file()
                    || margins_dir
                        .join(format!("{}_aligned.md", session.name))
                        .is_file())
            {
                updates.push(SessionArtifactPathUpdate {
                    session_name: session.name.clone(),
                    kind: artifact.kind,
                    ordinal: artifact.ordinal,
                    old_path: artifact.path,
                    new_path: format!(".margins/{}_aligned.md", session.name),
                });
            }
        }
    }

    let moves = by_target
        .into_iter()
        .map(|(target, source)| PlannedMove { source, target })
        .collect();
    Ok((moves, updates))
}

fn insert_move(
    by_target: &mut BTreeMap<PathBuf, PathBuf>,
    source: PathBuf,
    target: PathBuf,
) -> Result<()> {
    if let Some(existing) = by_target.get(&target) {
        if existing != &source {
            bail!(
                "multiple aligned transcripts would map to {}",
                target.display()
            );
        }
        return Ok(());
    }
    by_target.insert(target, source);
    Ok(())
}

fn preflight_targets(moves: &[PlannedMove]) -> Result<()> {
    for planned in moves {
        if planned.target.exists() && planned.target != planned.source {
            bail!(
                "Refusing to overwrite existing transcript {}.",
                planned.target.display()
            );
        }
    }
    Ok(())
}

fn rollback_moves(completed: &[PlannedMove]) {
    for planned in completed.iter().rev() {
        let _ = std::fs::rename(&planned.target, &planned.source);
    }
}

fn set_marker(work_dir: &Path, enabled: bool) -> Result<()> {
    let marker = archive_marker(work_dir);
    if enabled {
        let temporary = marker.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&temporary, b"1\n")?;
        std::fs::rename(&temporary, &marker)?;
    } else if marker.exists() {
        std::fs::remove_file(marker)?;
    }
    Ok(())
}

fn archive_marker(work_dir: &Path) -> PathBuf {
    work_dir.join(".margins").join(ARCHIVE_MARKER_NAME)
}

fn count_archived_transcripts(work_dir: &Path) -> Result<usize> {
    let dir = archive_path(work_dir);
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_aligned_markdown(&entry.path()) {
            count += 1;
        }
    }
    Ok(count)
}

fn is_aligned_markdown(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.ends_with("_aligned.md"))
}

fn validate_session_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if component == OsStr::new(name) => Ok(()),
        _ => bail!("invalid session name '{name}'"),
    }
}

fn reject_symlink(path: &Path) -> Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("Refusing to migrate through symlink {}.", path.display());
    }
    Ok(())
}
