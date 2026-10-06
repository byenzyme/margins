use anyhow::{bail, Context, Result};
use std::ffi::OsStr;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use margins_workflows::machine_config;
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Agent {
    ClaudeCode,
    Codex,
    Cursor,
}

pub(crate) const AGENT_SKILL_DIRS: [(Agent, &str); 3] = [
    (Agent::ClaudeCode, ".claude"),
    (Agent::Codex, ".codex"),
    (Agent::Cursor, ".cursor"),
];

impl Agent {
    pub(crate) fn display(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
        }
    }

    fn config_value(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        }
    }

    fn from_config_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claude code" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "cursor" | "cursor-agent" => Some(Self::Cursor),
            _ => None,
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor-agent",
        }
    }

    fn seed(self) -> &'static str {
        match self {
            Self::ClaudeCode => "/margins latest",
            Self::Codex | Self::Cursor => "distill my latest margins session",
        }
    }

    fn command_display(self) -> String {
        format!("{} {:?}", self.binary(), self.seed())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectionSource {
    HostAgent,
    RememberedDefault,
    SoleInstalled,
    Asked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AgentSelection {
    agent: Agent,
    source: SelectionSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentResolution {
    Resolved(AgentSelection),
    Ask,
}

fn resolve_agent(
    host: Option<Agent>,
    remembered: Option<Agent>,
    installed: &[Agent],
    answer: Option<Agent>,
) -> AgentResolution {
    if let Some(agent) = host {
        return AgentResolution::Resolved(AgentSelection {
            agent,
            source: SelectionSource::HostAgent,
        });
    }
    if let Some(agent) = remembered {
        return AgentResolution::Resolved(AgentSelection {
            agent,
            source: SelectionSource::RememberedDefault,
        });
    }
    if let [agent] = installed {
        return AgentResolution::Resolved(AgentSelection {
            agent: *agent,
            source: SelectionSource::SoleInstalled,
        });
    }
    match answer {
        Some(agent) => AgentResolution::Resolved(AgentSelection {
            agent,
            source: SelectionSource::Asked,
        }),
        None => AgentResolution::Ask,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoteAction {
    PrintCommand,
    PrintDirective,
    Exec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NotePlan {
    selection: AgentSelection,
    action: NoteAction,
}

fn plan_note(selection: AgentSelection, dry_run: bool, cli_available: bool) -> NotePlan {
    let action = if dry_run {
        NoteAction::PrintCommand
    } else if selection.source == SelectionSource::HostAgent || !cli_available {
        NoteAction::PrintDirective
    } else {
        NoteAction::Exec
    };
    NotePlan { selection, action }
}

fn render_note_output(plan: NotePlan) -> String {
    let agent = plan.selection.agent;
    match plan.action {
        NoteAction::PrintCommand => format!(
            "Agent: {}\nCommand: {}\n",
            agent.display(),
            agent.command_display()
        ),
        NoteAction::PrintDirective => format!(
            "Agent: {}\nContinue in {} with: {}\n",
            agent.display(),
            agent.display(),
            agent.seed()
        ),
        NoteAction::Exec => String::new(),
    }
}

fn detect_host_agent(keys: &[String]) -> Option<Agent> {
    let claude = keys.iter().any(|key| key == "CLAUDECODE");
    let codex = keys.iter().any(|key| key.starts_with("CODEX_"));
    let cursor = keys.iter().any(|key| key.starts_with("CURSOR_"));
    if claude {
        Some(Agent::ClaudeCode)
    } else if codex {
        Some(Agent::Codex)
    } else if cursor {
        Some(Agent::Cursor)
    } else {
        None
    }
}

fn margins_home(home: &Path) -> PathBuf {
    std::env::var_os("MARGINS_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".margins"))
}

fn read_remembered_agent(margins_home: &Path) -> Result<Option<Agent>> {
    let path = machine_config::machine_config_path(margins_home);
    let Some(contents) = machine_config::read_machine_config_text(margins_home)? else {
        return Ok(None);
    };
    if contents.trim().is_empty() {
        return Ok(None);
    }
    let document = contents
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let Some(value) = document
        .get("cli")
        .and_then(Item::as_table)
        .and_then(|table| table.get("note_agent"))
        .and_then(Item::as_str)
    else {
        return Ok(None);
    };
    Agent::from_config_value(value).map(Some).with_context(|| {
        format!(
            "unsupported cli.note_agent {:?} in {}",
            value,
            path.display()
        )
    })
}

fn remember_agent(margins_home: &Path, agent: Agent) -> Result<()> {
    let path = machine_config::machine_config_path(margins_home);
    machine_config::update_machine_document(margins_home, |document| {
        if document.get("cli").is_none() {
            document["cli"] = Item::Table(Table::new());
        }
        let cli = document["cli"]
            .as_table_mut()
            .with_context(|| format!("cli must be a table in {}", path.display()))?;
        cli["note_agent"] = value(agent.config_value());
        Ok(())
    })
}

fn executable_on_path(agent: Agent, path: Option<&OsStr>) -> bool {
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(path).any(|directory| is_executable(&directory.join(agent.binary())))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn installed_agents(home: &Path, path: Option<&OsStr>) -> Vec<Agent> {
    AGENT_SKILL_DIRS
        .iter()
        .filter_map(|(agent, directory)| {
            (home.join(directory).is_dir() && executable_on_path(*agent, path)).then_some(*agent)
        })
        .collect()
}

fn ask_for_agent(input: &mut dyn BufRead, output: &mut dyn Write) -> Result<Agent> {
    writeln!(output, "Which agent should write your notes?")?;
    writeln!(output, "  1. Claude Code")?;
    writeln!(output, "  2. Codex")?;
    writeln!(output, "  3. Cursor")?;
    write!(output, "> ")?;
    output.flush()?;

    let mut answer = String::new();
    input.read_line(&mut answer)?;
    match answer.trim().to_ascii_lowercase().as_str() {
        "1" | "claude" | "claude code" | "claude-code" => Ok(Agent::ClaudeCode),
        "2" | "codex" => Ok(Agent::Codex),
        "3" | "cursor" | "cursor-agent" => Ok(Agent::Cursor),
        _ => bail!("choose Claude Code, Codex, or Cursor"),
    }
}

pub(crate) fn run(print_only: bool) -> Result<()> {
    let environment_keys = std::env::vars_os()
        .map(|(key, _)| key.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let host = detect_host_agent(&environment_keys);
    let home = crate::cli::home_dir().context("could not resolve HOME for agent selection")?;
    let margins_home = margins_home(&home);
    let remembered = if host.is_some() {
        None
    } else {
        read_remembered_agent(&margins_home)?
    };
    let path = std::env::var_os("PATH");
    let installed = if host.is_some() || remembered.is_some() {
        Vec::new()
    } else {
        installed_agents(&home, path.as_deref())
    };

    let mut resolution = resolve_agent(host, remembered, &installed, None);
    if resolution == AgentResolution::Ask {
        let answer = ask_for_agent(&mut io::stdin().lock(), &mut io::stderr().lock())?;
        remember_agent(&margins_home, answer)?;
        resolution = resolve_agent(host, remembered, &installed, Some(answer));
    }
    let AgentResolution::Resolved(selection) = resolution else {
        unreachable!("the interactive answer resolves agent selection")
    };
    let cli_available = executable_on_path(selection.agent, path.as_deref());
    let plan = plan_note(selection, print_only, cli_available);

    match plan.action {
        NoteAction::PrintCommand | NoteAction::PrintDirective => {
            print!("{}", render_note_output(plan));
            io::stdout().flush()?;
            Ok(())
        }
        NoteAction::Exec => exec_agent(selection.agent),
    }
}

fn exec_agent(agent: Agent) -> Result<()> {
    let _ = crossterm::terminal::disable_raw_mode();
    io::stdout().flush()?;
    io::stderr().flush()?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let error = ProcessCommand::new(agent.binary()).arg(agent.seed()).exec();
        Err(error).with_context(|| format!("failed to exec {}", agent.binary()))
    }

    #[cfg(not(unix))]
    {
        let status = ProcessCommand::new(agent.binary())
            .arg(agent.seed())
            .status()
            .with_context(|| format!("failed to launch {}", agent.binary()))?;
        if status.success() {
            Ok(())
        } else {
            bail!("{} exited with {}", agent.binary(), status)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(agent: Agent, source: SelectionSource) -> AgentSelection {
        AgentSelection { agent, source }
    }

    #[test]
    fn seed_map_is_agent_specific_and_preserves_interactive_commands() {
        assert_eq!(Agent::ClaudeCode.seed(), "/margins latest");
        assert_eq!(
            Agent::ClaudeCode.command_display(),
            "claude \"/margins latest\""
        );
        assert_eq!(
            Agent::Codex.command_display(),
            "codex \"distill my latest margins session\""
        );
        assert_eq!(
            Agent::Cursor.command_display(),
            "cursor-agent \"distill my latest margins session\""
        );
        for command in [
            Agent::ClaudeCode.command_display(),
            Agent::Codex.command_display(),
            Agent::Cursor.command_display(),
        ] {
            assert!(!command.contains("--force"));
            assert!(!command.contains("--yolo"));
            assert!(!command.contains(" -p"));
        }
    }

    #[test]
    fn host_environment_detection_uses_verified_agent_markers() {
        assert_eq!(
            detect_host_agent(&["CLAUDECODE".into(), "CODEX_HOME".into()]),
            Some(Agent::ClaudeCode)
        );
        assert_eq!(
            detect_host_agent(&["CODEX_THREAD_ID".into()]),
            Some(Agent::Codex)
        );
        assert_eq!(
            detect_host_agent(&["CURSOR_SESSION".into()]),
            Some(Agent::Cursor)
        );
        assert_eq!(detect_host_agent(&["PATH".into()]), None);
    }

    #[test]
    fn resolution_precedence_is_host_then_default_then_sole_then_ask() {
        assert_eq!(
            resolve_agent(
                Some(Agent::Codex),
                Some(Agent::Cursor),
                &[Agent::ClaudeCode],
                Some(Agent::ClaudeCode),
            ),
            AgentResolution::Resolved(selection(Agent::Codex, SelectionSource::HostAgent))
        );
        assert_eq!(
            resolve_agent(
                None,
                Some(Agent::Cursor),
                &[Agent::ClaudeCode],
                Some(Agent::Codex),
            ),
            AgentResolution::Resolved(selection(Agent::Cursor, SelectionSource::RememberedDefault))
        );
        assert_eq!(
            resolve_agent(None, None, &[Agent::ClaudeCode], Some(Agent::Codex)),
            AgentResolution::Resolved(selection(Agent::ClaudeCode, SelectionSource::SoleInstalled))
        );
        assert_eq!(resolve_agent(None, None, &[], None), AgentResolution::Ask);
        assert_eq!(
            resolve_agent(None, None, &[Agent::ClaudeCode, Agent::Codex], None),
            AgentResolution::Ask
        );
        assert_eq!(
            resolve_agent(None, None, &[], Some(Agent::Codex)),
            AgentResolution::Resolved(selection(Agent::Codex, SelectionSource::Asked))
        );
    }

    #[test]
    fn in_agent_and_missing_path_both_print_instead_of_nesting_or_failing() {
        let in_agent = plan_note(
            selection(Agent::Codex, SelectionSource::HostAgent),
            false,
            true,
        );
        assert_eq!(in_agent.action, NoteAction::PrintDirective);

        let missing = plan_note(
            selection(Agent::ClaudeCode, SelectionSource::RememberedDefault),
            false,
            false,
        );
        assert_eq!(missing.action, NoteAction::PrintDirective);
        assert_eq!(
            render_note_output(missing),
            "Agent: Claude Code\nContinue in Claude Code with: /margins latest\n"
        );
    }

    #[test]
    fn print_output_names_agent_and_exact_command_without_launching() {
        let plan = plan_note(
            selection(Agent::Cursor, SelectionSource::RememberedDefault),
            true,
            true,
        );
        assert_eq!(plan.action, NoteAction::PrintCommand);
        assert_eq!(
            render_note_output(plan),
            "Agent: Cursor\nCommand: cursor-agent \"distill my latest margins session\"\n"
        );
    }

    #[test]
    fn remembered_agent_uses_cli_table_and_preserves_existing_config() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.toml"),
            "# existing policy\n[audio]\ninput_name = \"Mic\"\n\n[llm]\nlocal_model = \"fixture\"\n",
        )
        .unwrap();

        remember_agent(temp.path(), Agent::Codex).unwrap();

        assert_eq!(read_remembered_agent(temp.path()).unwrap(), Some(Agent::Codex));
        let saved = std::fs::read_to_string(temp.path().join("margins.toml")).unwrap();
        assert!(saved.contains("# existing policy"));
        assert!(saved.contains("input_name = \"Mic\""));
        assert!(saved.contains("[cli]"));
        assert!(saved.contains("note_agent = \"codex\""));
        let settings =
            std::fs::read_to_string(temp.path().join("configs/settings.enzyme")).unwrap();
        assert!(settings.contains("model \"fixture\""), "{settings}");
    }
}
