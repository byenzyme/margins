use crate::error::CliError;
use crate::output::line;
use std::io::Write;

pub fn workspace_setup(stdout: &mut dyn Write) -> Result<(), CliError> {
    let guide = margins_workflows::resources::margins_workspace_setup_guide();
    stdout
        .write_all(guide.as_bytes())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn onboarding(stdout: &mut dyn Write) -> Result<(), CliError> {
    stdout
        .write_all(margins_workflows::resources::MARGINS_GUIDED_ONBOARDING.as_bytes())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

/// Plain-language meanings of the words Margins prints, for `margins guide
/// glossary`.
pub const GLOSSARY: &str = "\
Margins words, in plain language

Workspace   The notes Margins works with for one practice (one area of your
            work): which folders it reads, what it leaves out, and the one
            folder where it writes new notes.
program     Your Workspace written down as one editable text file,
            $MARGINS_HOME/configs/<id>.enzyme. Read it with
            `margins --workspace <id> workspace show --text`; change it with
            `margins --workspace <id> workspace edit`.
Source      A place a Workspace reads: your notes folder (its home, where new
            notes go), a read-only reference folder, recordings, or a
            connected account.
reading     One line of the program that tells Margins what to pay attention
            to, such as `learn questions from folder \"People\"`. \"Learns from
            People\" means that reading.
learn questions
            How the program words a reading: Margins learns the questions a
            folder, tag, or linked note keeps raising, so it can bring those
            notes back when they matter. `learn questions automatically` lets
            it also pick what your readings miss.
catalyst    One of those learned questions. Margins uses catalysts to find
            notes that a plain word search would miss.
profile     The kind of questions Margins develops for a reading, such as
            `about relationships` or `about decisions`. It is not a weight or
            a score.
plan, apply `workspace plan` shows exactly what a change to the program would
            do and saves it; `workspace apply` makes exactly that change.
init, sync  Build or refresh the index Margins searches. `recall` searches it.
enzyme      The engine that indexes and searches your notes. Margins ships its
            own copy, separate from any `enzyme` you install, and keeps its
            data in $MARGINS_HOME (never ~/.enzyme). `margins enzyme …` runs it.
";

pub fn glossary(stdout: &mut dyn Write) -> Result<(), CliError> {
    stdout
        .write_all(GLOSSARY.as_bytes())
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
