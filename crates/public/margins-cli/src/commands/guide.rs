use crate::error::CliError;
use crate::output::line;
use std::io::Write;

pub fn workspace_setup(stdout: &mut dyn Write) -> Result<(), CliError> {
    stdout
        .write_all(margins_workflows::resources::MARGINS_WORKSPACE_SETUP_GUIDE.as_bytes())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn onboarding(stdout: &mut dyn Write) -> Result<(), CliError> {
    stdout
        .write_all(margins_workflows::resources::MARGINS_GUIDED_ONBOARDING.as_bytes())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn setup_handoff(workspace: Option<&str>, stdout: &mut dyn Write) -> Result<(), CliError> {
    line(stdout, format_args!("Paste into your agent:")).map_err(CliError::from_anyhow)?;
    if let Some(workspace) = workspace.filter(|workspace| !workspace.trim().is_empty()) {
        line(
            stdout,
            format_args!(
                "Help me set up Margins workspace {workspace} so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end."
            ),
        )
        .map_err(CliError::from_anyhow)
    } else {
        line(
            stdout,
            format_args!(
                "Go to my notes folder, then help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end."
            ),
        )
        .map_err(CliError::from_anyhow)
    }
}

pub fn note_handoff(
    workspace: Option<&str>,
    print: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    if !print {
        line(stdout, format_args!("Continue in your coding agent with:"))
            .map_err(CliError::from_anyhow)?;
    }
    let workspace = workspace
        .filter(|workspace| !workspace.trim().is_empty())
        .map(|workspace| format!(" in workspace {workspace}"))
        .unwrap_or_else(|| " in my current Margins workspace".to_string());
    let instruction = format!(
        "Use the Margins skill to turn my latest Margins session into a connected note{workspace}."
    );
    line(stdout, format_args!("{instruction}")).map_err(CliError::from_anyhow)
}
