use std::path::Path;
use std::process::Command;

pub(crate) enum EditorPreference<'a> {
    System,
    Obsidian,
    Vscode,
    TextEdit,
    Custom(&'a str),
}

pub(crate) fn editor_preference(editor_command: Option<&str>) -> EditorPreference<'_> {
    let Some(command) = editor_command.map(str::trim).filter(|s| !s.is_empty()) else {
        return EditorPreference::System;
    };
    let lower = command.to_ascii_lowercase();
    match lower.as_str() {
        "system" | "default" | "open" => EditorPreference::System,
        "obsidian" => EditorPreference::Obsidian,
        "vscode" | "vs code" | "visual studio code" | "code" => EditorPreference::Vscode,
        "textedit" => EditorPreference::TextEdit,
        _ if lower.starts_with("code ") => EditorPreference::Vscode,
        _ if lower.contains("textedit") => EditorPreference::TextEdit,
        _ => EditorPreference::Custom(command),
    }
}

pub(crate) fn open_path_with_editor(
    path: &Path,
    editor_command: Option<&str>,
) -> Result<(), String> {
    match editor_preference(editor_command) {
        EditorPreference::System => {
            Command::new("open")
                .arg(path)
                .status()
                .map_err(|e| format!("failed to open note: {e}"))?;
            Ok(())
        }
        EditorPreference::Obsidian => {
            Err("Obsidian requires a saved note inside the configured notes folder.".to_string())
        }
        EditorPreference::Vscode => open_path_in_vscode(path),
        EditorPreference::TextEdit => {
            let status = Command::new("open")
                .arg("-a")
                .arg("TextEdit")
                .arg(path)
                .status()
                .map_err(|e| format!("failed to open TextEdit: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("TextEdit exited with {status}"))
            }
        }
        EditorPreference::Custom(command) => {
            let quoted_path = shell_quote(path.to_string_lossy().as_ref());
            let script = if command.contains("{file}") {
                command.replace("{file}", &quoted_path)
            } else {
                format!("{} {}", command, quoted_path)
            };
            let status = Command::new("sh")
                .arg("-lc")
                .arg(script)
                .status()
                .map_err(|e| format!("failed to run editor command: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("editor command exited with {status}"))
            }
        }
    }
}

fn open_path_in_vscode(path: &Path) -> Result<(), String> {
    let folder = path.parent().unwrap_or_else(|| Path::new("."));
    let status = Command::new("code")
        .arg(folder)
        .arg("--goto")
        .arg(path)
        .status()
        .map_err(|e| {
            format!(
                "failed to open VS Code. Install the 'code' command or choose another editor: {e}"
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("VS Code exited with {status}"))
    }
}

fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\\''"))
}

pub(crate) fn uri_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pref_name(command: Option<&str>) -> &'static str {
        match editor_preference(command) {
            EditorPreference::System => "system",
            EditorPreference::Obsidian => "obsidian",
            EditorPreference::Vscode => "vscode",
            EditorPreference::TextEdit => "textedit",
            EditorPreference::Custom(_) => "custom",
        }
    }

    #[test]
    fn classifies_editor_preferences() {
        assert_eq!(pref_name(None), "system");
        assert_eq!(pref_name(Some(" default ")), "system");
        assert_eq!(pref_name(Some("Obsidian")), "obsidian");
        assert_eq!(pref_name(Some("code --reuse-window")), "vscode");
        assert_eq!(pref_name(Some("open -a TextEdit")), "textedit");
        assert_eq!(pref_name(Some("mate")), "custom");
    }

    #[test]
    fn encodes_obsidian_uri_components() {
        assert_eq!(uri_encode("My Vault/file name"), "My%20Vault%2Ffile%20name");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("Bob's Note.md"), "'Bob'\\''s Note.md'");
    }
}
