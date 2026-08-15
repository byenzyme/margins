use crate::error::CliError;
use crate::output::{line, xml_escape_attr};
use margins_workflows::project::ResolvedProject;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Establish a Margins vault at `dir` (or the invocation directory), git-style.
/// This helper remains public for tests and embedding, but product init is owned
/// by the official recall-capable composition.
pub fn establish(
    invocation_dir: &Path,
    dir: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let root = establish_root(invocation_dir, dir)?;
    write_init(stdout, &root, "ok", None)
}

/// Establish the vault without publishing a final status. The official binary
/// uses this form so it can provision recall before reporting whether init was
/// fully ready.
pub fn establish_root(invocation_dir: &Path, dir: Option<&Path>) -> Result<PathBuf, CliError> {
    let cwd = match dir {
        Some(path) => absolute_from(invocation_dir, path),
        None => invocation_dir.to_path_buf(),
    };
    let root = margins_workflows::project::ensure_vault(&cwd).map_err(CliError::from_anyhow)?;
    Ok(root)
}

pub fn write_init(
    stdout: &mut dyn Write,
    root: &Path,
    status: &str,
    config_path: Option<&Path>,
) -> Result<(), CliError> {
    let config_attr = config_path
        .map(|path| {
            format!(
                " config_path=\"{}\"",
                xml_escape_attr(&path.to_string_lossy())
            )
        })
        .unwrap_or_default();
    line(
        stdout,
        format_args!(
            "<margins_init path=\"{}\" status=\"{}\"{} />",
            xml_escape_attr(&root.to_string_lossy()),
            xml_escape_attr(status),
            config_attr,
        ),
    )
    .map_err(CliError::from_anyhow)
}

pub fn install_agents(work_dir: &Path, stdout: &mut dyn Write) -> Result<(), CliError> {
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = work_dir.join(name);
        let action = margins_workflows::agents::write_agent_instruction_file(&path)
            .map_err(CliError::from_anyhow)?;
        line(stdout, format_args!("{} {}", action.as_str(), name))
            .map_err(CliError::from_anyhow)?;
    }
    Ok(())
}

pub fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[allow(dead_code)]
fn _assert_resolved_project_send_sync(_: &ResolvedProject) {}
