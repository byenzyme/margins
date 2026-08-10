use crate::error::CliError;
use crate::output::{line, xml_escape_attr};
use crate::services::CliServices;
use margins_workflows::project::{project_to_xml, ResolvedProject};
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn project_list(services: &CliServices, stdout: &mut dyn Write) -> Result<(), CliError> {
    list(services, false, stdout)
}

pub fn project_current(services: &CliServices, stdout: &mut dyn Write) -> Result<(), CliError> {
    let project = services
        .projects
        .resolve(None)
        .map_err(CliError::from_anyhow)?;
    line(stdout, format_args!("<margins_project_current>")).map_err(CliError::from_anyhow)?;
    write!(stdout, "{}", project_to_xml(&project, true))
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    line(stdout, format_args!("</margins_project_current>")).map_err(CliError::from_anyhow)
}

pub fn project_use(
    services: &CliServices,
    selector: &str,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let project = services
        .projects
        .set_active(selector)
        .map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "<margins_project_selected id=\"{}\">",
            xml_escape_attr(&project.project.id)
        ),
    )
    .map_err(CliError::from_anyhow)?;
    write!(stdout, "{}", project_to_xml(&project, true))
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    line(stdout, format_args!("</margins_project_selected>")).map_err(CliError::from_anyhow)
}

pub fn list(services: &CliServices, json: bool, stdout: &mut dyn Write) -> Result<(), CliError> {
    let active = services.projects.resolve(None).ok();
    let projects = services.projects.list().map_err(CliError::from_anyhow)?;
    if json {
        let rows = projects
            .iter()
            .map(|project| {
                let active = active
                    .as_ref()
                    .is_some_and(|active| active.project.id == project.project.id);
                serde_json::json!({
                    "id": project.project.id,
                    "name": project.project.name,
                    "path": project.root_dir.to_string_lossy(),
                    "inbox_folder": project.project.inbox_folder,
                    "people_folder": project.project.people_folder,
                    "readiness": project.project.readiness,
                    "active": active,
                })
            })
            .collect::<Vec<_>>();
        line(
            stdout,
            format_args!(
                "{}",
                serde_json::to_string_pretty(&rows)
                    .map_err(|error| CliError::new("output_failed", error.to_string()))?
            ),
        )
        .map_err(CliError::from_anyhow)?;
        return Ok(());
    }
    line(stdout, format_args!("<margins_projects>")).map_err(CliError::from_anyhow)?;
    for project in projects {
        let is_active = active
            .as_ref()
            .is_some_and(|active| active.project.id == project.project.id);
        write!(stdout, "{}", project_to_xml(&project, is_active))
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    }
    line(stdout, format_args!("</margins_projects>")).map_err(CliError::from_anyhow)
}

pub fn add(
    services: &CliServices,
    invocation_dir: &Path,
    path: &str,
    name: Option<&str>,
    inbox_folder: Option<&str>,
    init: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError> {
    let path = project_path_from(invocation_dir, path);
    let project = services
        .projects
        .add(&path, name, inbox_folder)
        .map_err(CliError::from_anyhow)?;
    margins_workflows::project::install_local_setup_skill(&project.root_dir)
        .map_err(CliError::from_anyhow)?;
    if init {
        if let Err(error) = run_enzyme(services, &project.root_dir) {
            line(stderr, format_args!("Warning: enzyme init failed: {error}"))
                .map_err(CliError::from_anyhow)?;
        }
    }
    line(stdout, format_args!("<margins_project_added>")).map_err(CliError::from_anyhow)?;
    write!(stdout, "{}", project_to_xml(&project, true))
        .map_err(|error| CliError::new("output_failed", error.to_string()))?;
    line(stdout, format_args!("</margins_project_added>")).map_err(CliError::from_anyhow)
}

pub fn init(
    services: &CliServices,
    invocation_dir: &Path,
    path: Option<&Path>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let root = if let Some(path) = path {
        let path = absolute_from(invocation_dir, path);
        path.canonicalize().map_err(|_| {
            CliError::new(
                "project_not_found",
                format!("Path does not exist: {}", path.display()),
            )
        })?
    } else {
        services
            .projects
            .resolve(None)
            .map_err(CliError::from_anyhow)?
            .root_dir
    };
    std::fs::create_dir_all(root.join(".margins"))
        .map_err(|error| CliError::new("store_failed", error.to_string()))?;
    margins_workflows::project::install_local_setup_skill(&root).map_err(CliError::from_anyhow)?;
    run_enzyme(services, &root)?;
    line(
        stdout,
        format_args!(
            "<margins_projects_init path=\"{}\" status=\"ok\" />",
            xml_escape_attr(&root.to_string_lossy())
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

fn run_enzyme(services: &CliServices, root: &Path) -> Result<(), CliError> {
    let arguments = vec![
        "-p".to_string(),
        root.to_string_lossy().into_owned(),
        "init".to_string(),
    ];
    let status = services
        .processes
        .run(OsStr::new("enzyme"), &arguments, root)
        .map_err(CliError::from_anyhow)?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::new(
            "process_failed",
            format!("enzyme init exited with {status}"),
        ))
    }
}

pub fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn project_path_from(invocation_dir: &Path, path: &str) -> String {
    let candidate = Path::new(path);
    if candidate.is_absolute() || path == "~" || path.starts_with("~/") {
        path.to_string()
    } else {
        invocation_dir
            .join(candidate)
            .to_string_lossy()
            .into_owned()
    }
}

#[allow(dead_code)]
fn _assert_resolved_project_send_sync(_: &ResolvedProject) {}
