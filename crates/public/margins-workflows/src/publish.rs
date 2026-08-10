//! Portable vault publishing and template rendering.
use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use regex::Regex;
use serde::Deserialize;
use std::path::Path;

#[derive(Deserialize)]
pub struct MarginsConfig {
    pub vault: VaultConfig,
}

#[derive(Deserialize)]
pub struct VaultConfig {
    pub path: String,
    pub folder: String,
    pub filename: Option<String>,
    pub template: Option<String>,
    pub open_in_obsidian: Option<bool>,
}

const DEFAULT_TEMPLATE: &str = "\
---
created: '[[{{date:%Y-%m-%d}}]]'
margins_session: {{name}}
---

{{memo}}";

const DEFAULT_FILENAME: &str = "{{date:%Y-%m-%d-%-H-%M-%S}}";

pub fn load_config(margins_dir: &Path) -> Result<Option<MarginsConfig>> {
    let config_path = margins_dir.join("config.toml");
    if !config_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("failed to read {:?}", config_path))?;
    let config: MarginsConfig =
        toml::from_str(&content).with_context(|| format!("failed to parse {:?}", config_path))?;
    Ok(Some(config))
}

pub fn render_template(
    template: &str,
    name: &str,
    memo: &str,
    start_time: &DateTime<Local>,
    duration_secs: f64,
) -> String {
    let duration = format_duration(duration_secs);

    let result = template.replace("{{name}}", name);
    let result = result.replace("{{memo}}", memo);
    let result = result.replace("{{duration}}", &duration);

    // Replace {{date:FORMAT}} patterns
    let date_re = Regex::new(r"\{\{date:([^}]+)\}\}").unwrap();
    let result = date_re
        .replace_all(&result, |caps: &regex::Captures| {
            let fmt = &caps[1];
            start_time.format(fmt).to_string()
        })
        .to_string();

    result
}

pub fn publish_to_vault(
    margins_dir: &Path,
    name: &str,
    memo: &str,
    start_time: &DateTime<Local>,
    duration_secs: f64,
) -> Result<Option<String>> {
    if memo.trim().is_empty() {
        return Ok(None);
    }

    let config = match load_config(margins_dir) {
        Ok(Some(c)) => c,
        Ok(None) => return Ok(None),
        Err(e) => {
            eprintln!("Warning: failed to load margins config: {}", e);
            return Ok(None);
        }
    };

    // Expand ~ in vault path
    let vault_path = expand_tilde(&config.vault.path);
    let vault_dir = Path::new(&vault_path).join(&config.vault.folder);

    std::fs::create_dir_all(&vault_dir)
        .with_context(|| format!("failed to create vault directory {:?}", vault_dir))?;

    // Render filename
    let filename_pattern = config.vault.filename.as_deref().unwrap_or(DEFAULT_FILENAME);
    let filename = render_template(filename_pattern, name, "", start_time, duration_secs);
    let filename = format!("{}.md", filename);

    // Load template
    let template_content = if let Some(ref template_file) = config.vault.template {
        let template_path = margins_dir.join(template_file);
        match std::fs::read_to_string(&template_path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!(
                    "Warning: failed to read template {:?}: {}. Using default.",
                    template_path, e
                );
                DEFAULT_TEMPLATE.to_string()
            }
        }
    } else {
        DEFAULT_TEMPLATE.to_string()
    };

    // Render template
    let rendered = render_template(&template_content, name, memo, start_time, duration_secs);

    // Write to vault
    let note_path = vault_dir.join(&filename);
    if !std::fs::read(&note_path).is_ok_and(|existing| existing == rendered.as_bytes()) {
        std::fs::write(&note_path, &rendered)
            .with_context(|| format!("failed to write vault note {:?}", note_path))?;
    }

    let vault_note_path = note_path.to_string_lossy().to_string();

    // Open in Obsidian if configured
    if config.vault.open_in_obsidian.unwrap_or(false) {
        let vault_name = Path::new(&vault_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("vault");
        let relative_path = format!("{}/{}", config.vault.folder, filename);
        let uri = format!(
            "obsidian://open?vault={}&file={}",
            urlencod(vault_name),
            urlencod(&relative_path)
        );
        if let Err(e) = std::process::Command::new("open").arg(&uri).spawn() {
            eprintln!("Warning: failed to open Obsidian: {}", e);
        }
    }

    Ok(Some(vault_note_path))
}

fn format_duration(secs: f64) -> String {
    let total = secs as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), &path[2..]);
        }
    }
    path.to_string()
}

/// Minimal percent-encoding for Obsidian URI components.
fn urlencod(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '#' => out.push_str("%23"),
            '&' => out.push_str("%26"),
            '?' => out.push_str("%3F"),
            '%' => out.push_str("%25"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[cfg(unix)]
    #[test]
    fn identical_republish_does_not_rewrite_the_note() {
        let temp = tempfile::tempdir().unwrap();
        let margins_dir = temp.path().join(".margins");
        let vault = temp.path().join("vault");
        std::fs::create_dir_all(&margins_dir).unwrap();
        std::fs::write(
            margins_dir.join("config.toml"),
            format!(
                "[vault]\npath = {:?}\nfolder = \"meetings\"\nfilename = \"fixed\"\n",
                vault.to_string_lossy()
            ),
        )
        .unwrap();
        let started = Local.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap();
        let first = publish_to_vault(&margins_dir, "meet", "memo", &started, 1.0)
            .unwrap()
            .unwrap();
        let path = Path::new(&first);
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(path, permissions).unwrap();

        assert_eq!(
            publish_to_vault(&margins_dir, "meet", "memo", &started, 1.0)
                .unwrap()
                .as_deref(),
            Some(first.as_str())
        );
    }
}
