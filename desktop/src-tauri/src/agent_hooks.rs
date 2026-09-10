// agent_hooks.rs — install the Margins meeting-awareness hook into a coding
// agent's project configuration.
//
// The hook (a Node script bundled as an app resource) runs on every user turn
// of Claude Code or Codex CLI and injects a note when a meeting is being
// captured or the durable live transcript has advanced past what the agent
// previously fetched. Installation is directory-scoped: the script is copied
// into `<dir>/.margins/hooks/` and referenced with an absolute path from
// `<dir>/.claude/settings.json` and `<dir>/.codex/hooks.json`, so each project
// opts in independently.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HOOK_SCRIPT: &str = include_str!("../resources/hooks/margins-agent-hook.mjs");
/// Marker used to detect an already-installed hook entry regardless of the
/// absolute path it was installed under.
const HOOK_MARKER: &str = "margins-agent-hook.mjs";
/// Codex gates its hook runtime behind this experimental feature flag
/// (`codex_hooks` is a deprecated alias as of codex-cli 0.144).
const CODEX_HOOKS_FEATURE_KEY: &str = "hooks";

#[derive(Serialize, Default)]
pub(crate) struct AgentHooksInstallResult {
    pub(crate) hook_script_path: String,
    pub(crate) claude_settings_path: Option<String>,
    pub(crate) claude_already_installed: bool,
    pub(crate) codex_hooks_path: Option<String>,
    pub(crate) codex_config_path: Option<String>,
    pub(crate) codex_already_installed: bool,
}

/// Install the hook for the requested agents (`claude`, `codex`; both when
/// `agents` is `None`) into `dir`.
pub(crate) fn install_agent_hooks_impl(
    dir: String,
    agents: Option<Vec<String>>,
) -> Result<AgentHooksInstallResult, String> {
    let dir = PathBuf::from(dir);
    if !dir.is_dir() {
        return Err(format!(
            "Target directory does not exist: {}",
            dir.display()
        ));
    }
    let agents = agents.unwrap_or_else(|| vec!["claude".to_string(), "codex".to_string()]);
    for agent in &agents {
        if agent != "claude" && agent != "codex" {
            return Err(format!("Unknown agent '{agent}' (expected claude or codex)"));
        }
    }

    let script_path = write_hook_script(&dir)?;
    let command = format!("node \"{}\"", script_path.display());

    let mut result = AgentHooksInstallResult {
        hook_script_path: script_path.display().to_string(),
        ..Default::default()
    };

    if agents.iter().any(|a| a == "claude") {
        let settings_path = dir.join(".claude").join("settings.json");
        result.claude_already_installed = !merge_hook_into_settings(&settings_path, &command)?;
        result.claude_settings_path = Some(settings_path.display().to_string());
    }

    if agents.iter().any(|a| a == "codex") {
        let hooks_path = dir.join(".codex").join("hooks.json");
        result.codex_already_installed = !merge_hook_into_settings(&hooks_path, &command)?;
        result.codex_hooks_path = Some(hooks_path.display().to_string());
        let config_path = dir.join(".codex").join("config.toml");
        enable_codex_hooks_feature(&config_path)?;
        result.codex_config_path = Some(config_path.display().to_string());
    }

    Ok(result)
}

fn write_hook_script(dir: &Path) -> Result<PathBuf, String> {
    let hooks_dir = dir.join(".margins").join("hooks");
    std::fs::create_dir_all(&hooks_dir)
        .map_err(|e| format!("Could not create {}: {e}", hooks_dir.display()))?;
    let script_path = hooks_dir.join(HOOK_MARKER);
    std::fs::write(&script_path, HOOK_SCRIPT)
        .map_err(|e| format!("Could not write {}: {e}", script_path.display()))?;
    Ok(script_path)
}

/// Merge a UserPromptSubmit command hook into a hook config file. Claude
/// Code's `settings.json` and Codex's `hooks.json` share the same shape:
/// `{"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", ...}]}]}}`.
/// Returns `Ok(true)` when an entry was added, `Ok(false)` when an equivalent
/// hook was already present.
fn merge_hook_into_settings(path: &Path, command: &str) -> Result<bool, String> {
    let mut root: Value = match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?,
        Err(_) => json!({}),
    };
    if !root.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }

    let events = ensure_object_entry(&mut root, "hooks")?;
    let entries = events
        .as_object_mut()
        .unwrap()
        .entry("UserPromptSubmit")
        .or_insert_with(|| json!([]));
    let Some(entries) = entries.as_array_mut() else {
        return Err(format!(
            "hooks.UserPromptSubmit in {} is not an array",
            path.display()
        ));
    };

    let already_installed = entries.iter().any(|matcher| {
        matcher["hooks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|hook| {
                hook["command"]
                    .as_str()
                    .is_some_and(|c| c.contains(HOOK_MARKER))
            })
    });
    if already_installed {
        return Ok(false);
    }

    entries.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 10,
        }]
    }));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(path, raw + "\n")
        .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(true)
}

fn ensure_object_entry<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Value, String> {
    let obj = value
        .as_object_mut()
        .ok_or_else(|| format!("expected a JSON object around '{key}'"))?;
    let entry = obj.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        return Err(format!("'{key}' exists but is not a JSON object"));
    }
    Ok(entry)
}

/// Codex only runs hooks when the experimental feature flag is on; flip it in
/// the project-scoped config so the global config stays untouched.
fn enable_codex_hooks_feature(path: &Path) -> Result<(), String> {
    let mut root: toml::Value = match std::fs::read_to_string(path) {
        Ok(raw) => raw
            .parse()
            .map_err(|e| format!("{} is not valid TOML: {e}", path.display()))?,
        Err(_) => toml::Value::Table(Default::default()),
    };
    let table = root
        .as_table_mut()
        .ok_or_else(|| format!("{} is not a TOML table", path.display()))?;
    let features = table
        .entry("features")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    let features = features
        .as_table_mut()
        .ok_or_else(|| format!("'features' in {} is not a table", path.display()))?;
    if features.get(CODEX_HOOKS_FEATURE_KEY).and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    features.insert(CODEX_HOOKS_FEATURE_KEY.to_string(), toml::Value::Boolean(true));

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(&root).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| format!("Could not write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("margins-agent-hooks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn installs_into_empty_directory_for_both_agents() {
        let dir = temp_dir("empty");
        let result = install_agent_hooks_impl(dir.display().to_string(), None).unwrap();
        assert!(!result.claude_already_installed);
        assert!(!result.codex_already_installed);

        let script = dir.join(".margins/hooks/margins-agent-hook.mjs");
        assert!(script.is_file());

        let claude: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude/settings.json")).unwrap())
                .unwrap();
        let command = claude["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains(HOOK_MARKER));

        let codex: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".codex/hooks.json")).unwrap())
                .unwrap();
        assert!(codex["hooks"]["hooks"].is_null(), "codex hooks must not double-nest");
        let command = codex["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.contains(HOOK_MARKER));

        let config = std::fs::read_to_string(dir.join(".codex/config.toml")).unwrap();
        assert!(config.contains(CODEX_HOOKS_FEATURE_KEY));
    }

    #[test]
    fn preserves_existing_settings_and_is_idempotent() {
        let dir = temp_dir("merge");
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(
            dir.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(ls:*)"]},"hooks":{"PostToolUse":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(dir.join(".codex/config.toml"), "model = \"gpt-5.2\"\n").unwrap();

        let first = install_agent_hooks_impl(dir.display().to_string(), None).unwrap();
        assert!(!first.claude_already_installed);
        let second = install_agent_hooks_impl(dir.display().to_string(), None).unwrap();
        assert!(second.claude_already_installed);
        assert!(second.codex_already_installed);

        let claude: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(".claude/settings.json")).unwrap())
                .unwrap();
        assert_eq!(claude["permissions"]["allow"][0], "Bash(ls:*)");
        assert!(claude["hooks"]["PostToolUse"].is_array());
        assert_eq!(claude["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);

        let config = std::fs::read_to_string(dir.join(".codex/config.toml")).unwrap();
        assert!(config.contains("model = \"gpt-5.2\""));
        assert!(config.contains(CODEX_HOOKS_FEATURE_KEY));
    }

    #[test]
    fn rejects_missing_directory_and_unknown_agent() {
        let missing = std::env::temp_dir().join("margins-agent-hooks-definitely-missing");
        assert!(install_agent_hooks_impl(missing.display().to_string(), None).is_err());

        let dir = temp_dir("agents");
        assert!(
            install_agent_hooks_impl(dir.display().to_string(), Some(vec!["cursor".into()]))
                .is_err()
        );
    }
}
