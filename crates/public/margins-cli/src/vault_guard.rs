//! Safety checks for commands that must never infer a vault from a generic directory.

use crate::error::CliError;
use margins_workflows::project::ResolvedProject;
use std::path::{Path, PathBuf};

pub fn require_evidenced_vault(project: &ResolvedProject) -> Result<(), CliError> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    validate(project, home.as_deref(), &margins_config_dirs())
}

fn validate(
    project: &ResolvedProject,
    home: Option<&Path>,
    config_dirs: &[PathBuf],
) -> Result<(), CliError> {
    let vault = comparable(&project.root_dir);
    if vault == Path::new("/") {
        return Err(refusal("/"));
    }

    if let Some(home) = home {
        if vault == comparable(home) {
            return Err(CliError::new(
                "unsafe_vault",
                "refusing to treat $HOME as a vault; pass --vault or run from your notes folder",
            ));
        }
    }

    for config_dir in config_dirs {
        if vault == comparable(config_dir) {
            return Err(refusal(&config_dir.display().to_string()));
        }
    }

    let has_marker = vault.join(".obsidian").is_dir() || project.project.readiness != "needs_setup";
    if !has_marker && !contains_note(&vault) {
        return Err(CliError::new(
            "unsafe_vault",
            format!(
                "refusing to treat {} as a vault because it has no notes or Margins vault configuration; pass --vault or run from your notes folder",
                vault.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use margins_workflows::project::ProjectSource;

    fn project(root: &Path) -> ResolvedProject {
        ResolvedProject {
            project: ProjectSource {
                id: "test".into(),
                name: "test".into(),
                path: root.to_string_lossy().into_owned(),
                inbox_folder: "meetings".into(),
                people_folder: "people".into(),
                readiness: "needs_setup".into(),
            },
            root_dir: root.into(),
            work_dir: root.into(),
        }
    }

    #[test]
    fn refuses_home_as_vault() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate(&project(temp.path()), Some(temp.path()), &[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("refusing to treat $HOME as a vault"));
    }

    #[test]
    fn refuses_margins_config_dir_as_vault() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate(&project(temp.path()), None, &[temp.path().into()]).unwrap_err();
        assert!(error.to_string().contains("refusing to treat"));
    }

    #[test]
    fn refuses_filesystem_root_as_vault() {
        let error = validate(&project(Path::new("/")), None, &[]).unwrap_err();
        assert!(error.to_string().contains("refusing to treat / as a vault"));
    }

    #[test]
    fn refuses_directory_without_vault_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let error = validate(&project(temp.path()), None, &[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("no notes or Margins vault configuration"));
    }

    #[test]
    fn accepts_note_evidence() {
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("note.md"), "# note").unwrap();
        validate(&project(notes.path()), None, &[]).unwrap();
    }
}

fn refusal(path: &str) -> CliError {
    CliError::new(
        "unsafe_vault",
        format!("refusing to treat {path} as a vault; pass --vault or run from your notes folder"),
    )
}

fn margins_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("MARGINS_HOME").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(path));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        dirs.push(PathBuf::from(home).join(".margins"));
    }
    dirs
}

fn comparable(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn contains_note(root: &Path) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if entry.file_name() != ".margins" && entry.file_name() != ".git" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
            }) {
                return true;
            }
        }
    }
    false
}
