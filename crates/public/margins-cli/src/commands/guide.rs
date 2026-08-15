use crate::error::CliError;
use crate::output::line;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn workspace_setup(stdout: &mut dyn Write) -> Result<(), CliError> {
    stdout
        .write_all(margins_workflows::resources::MARGINS_WORKSPACE_SETUP_GUIDE.as_bytes())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn setup_handoff(invocation_dir: &Path, stdout: &mut dyn Write) -> Result<(), CliError> {
    let root = absolute_invocation_dir(invocation_dir);
    line(stdout, format_args!("Paste into your agent:")).map_err(CliError::from_anyhow)?;
    line(
        stdout,
        format_args!(
            "Set up Margins in {}. Run margins guide workspace-setup and follow it end to end.",
            root.display()
        ),
    )
    .map_err(CliError::from_anyhow)
}

fn absolute_invocation_dir(invocation_dir: &Path) -> PathBuf {
    if invocation_dir.is_absolute() {
        invocation_dir.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(invocation_dir)
    }
}
