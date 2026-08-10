use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::resources::{
    ENZYME_WORKSPACE_SETUP_SKILL as WORKSPACE_SETUP_SKILL, MARGINS_WORKSPACE_SETUP_SKILL,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProjectSource {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_inbox_folder")]
    pub inbox_folder: String,
    #[serde(default = "default_people_folder")]
    pub people_folder: String,
    #[serde(default = "default_project_readiness")]
    pub readiness: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProjectSettings {
    #[serde(default)]
    vault_path: Option<String>,
    #[serde(default)]
    projects: Vec<ProjectSource>,
    #[serde(default)]
    active_project_id: Option<String>,
    #[serde(default = "default_inbox_folder")]
    inbox_folder: String,
    #[serde(default = "default_people_folder")]
    people_folder: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedProject {
    pub project: ProjectSource,
    pub root_dir: PathBuf,
    pub work_dir: PathBuf,
}

pub fn inbox_dir(project: &ResolvedProject) -> PathBuf {
    project.root_dir.join(project.project.inbox_folder.trim())
}

pub fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(profile_slug())
        .join("settings.json")
}

pub fn list_projects() -> Result<Vec<ResolvedProject>> {
    let settings = load_project_settings()?;
    Ok(normalize_projects(&settings)
        .into_iter()
        .map(resolve_project_paths)
        .collect())
}

pub fn resolve_project(selector: Option<&str>) -> Result<ResolvedProject> {
    let settings = load_project_settings()?;
    let projects = normalize_projects(&settings);
    let project = if let Some(selector) = selector.map(str::trim).filter(|s| !s.is_empty()) {
        find_project(&projects, selector).with_context(|| {
            format!(
                "Unknown Margins project '{selector}'. Run `margins project list` and choose a configured project."
            )
        })?
    } else {
        let active = settings.active_project_id.as_deref();
        active
            .and_then(|id| projects.iter().find(|project| project.id == id))
            .or_else(|| projects.first())
            .cloned()
            .context("No Margins projects are configured. Open Margins Desktop and choose a project folder first.")?
    };
    Ok(resolve_project_paths(project))
}

pub fn set_active_project(selector: &str) -> Result<ResolvedProject> {
    let path = settings_path();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read Margins settings at {}", path.display()))?;
    let mut value: Value =
        serde_json::from_str(&raw).context("Margins settings are not valid JSON")?;
    let settings: ProjectSettings = serde_json::from_value(value.clone())
        .context("Margins settings have invalid project data")?;
    let projects = normalize_projects(&settings);
    let project = find_project(&projects, selector).with_context(|| {
        format!(
            "Unknown Margins project '{selector}'. `margins project use` only accepts projects already configured in Margins Desktop."
        )
    })?;

    let obj = value
        .as_object_mut()
        .context("Margins settings root is not an object")?;
    obj.insert(
        "active_project_id".to_string(),
        Value::String(project.id.clone()),
    );
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid Margins settings path"))?,
    )?;
    std::fs::write(&path, serde_json::to_string_pretty(&value)?)
        .with_context(|| format!("Could not write Margins settings at {}", path.display()))?;
    Ok(resolve_project_paths(project))
}

fn load_project_settings() -> Result<ProjectSettings> {
    let path = settings_path();
    if !path.exists() {
        return Ok(ProjectSettings {
            vault_path: Some(default_vault_path_string()),
            projects: vec![default_project_source(
                &default_vault_path_string(),
                &default_inbox_folder(),
                &default_people_folder(),
            )],
            active_project_id: Some(project_id_from_path(&default_vault_path_string())),
            inbox_folder: default_inbox_folder(),
            people_folder: default_people_folder(),
        });
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read Margins settings at {}", path.display()))?;
    let settings: ProjectSettings =
        serde_json::from_str(&raw).context("Margins settings have invalid project data")?;
    Ok(settings)
}

fn normalize_projects(settings: &ProjectSettings) -> Vec<ProjectSource> {
    let mut projects = settings
        .projects
        .iter()
        .enumerate()
        .filter_map(|(idx, raw)| {
            let path = raw.path.trim().to_string().or_else_nonempty(|| {
                if idx == 0 {
                    settings.vault_path.clone()
                } else {
                    None
                }
            })?;
            let inbox = if raw.inbox_folder.trim().is_empty() {
                settings.inbox_folder.clone()
            } else {
                raw.inbox_folder.clone()
            };
            let people = if raw.people_folder.trim().is_empty() {
                settings.people_folder.clone()
            } else {
                raw.people_folder.clone()
            };
            Some(ProjectSource {
                id: raw
                    .id
                    .trim()
                    .to_string()
                    .or_else_nonempty(|| Some(project_id_from_path(&path)))?,
                name: raw
                    .name
                    .trim()
                    .to_string()
                    .or_else_nonempty(|| Some(project_name_from_path(&path)))?,
                path,
                inbox_folder: inbox,
                people_folder: if people.trim().is_empty() {
                    default_people_folder()
                } else {
                    people
                },
                readiness: if raw.readiness.trim().is_empty() {
                    default_project_readiness()
                } else {
                    raw.readiness.clone()
                },
            })
        })
        .collect::<Vec<_>>();
    if projects.is_empty() {
        let root = settings
            .vault_path
            .clone()
            .unwrap_or_else(default_vault_path_string);
        projects.push(default_project_source(
            &root,
            &settings.inbox_folder,
            &settings.people_folder,
        ));
    }
    projects
}

fn find_project(projects: &[ProjectSource], selector: &str) -> Option<ProjectSource> {
    let selector = selector.trim();
    let expanded = expand_tilde(selector);
    let selector_path = PathBuf::from(&expanded);
    let selector_canon = canonicalize_for_compare(&selector_path);
    projects.iter().find_map(|project| {
        let root = PathBuf::from(expand_tilde(&project.path));
        let work = root.join(project.inbox_folder.trim());
        let path_match = canonicalize_for_compare(&root) == selector_canon
            || canonicalize_for_compare(&work) == selector_canon;
        if project.id == selector
            || project.name == selector
            || project.path == selector
            || path_match
        {
            Some(project.clone())
        } else {
            None
        }
    })
}

fn resolve_project_paths(project: ProjectSource) -> ResolvedProject {
    let root_dir = PathBuf::from(expand_tilde(&project.path));
    let work_dir = root_dir.clone();
    ResolvedProject {
        project,
        root_dir,
        work_dir,
    }
}

fn canonicalize_for_compare(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn profile_name() -> String {
    std::env::var("MARGINS_PROFILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "default")
        .unwrap_or_else(|| "default".to_string())
}

fn profile_slug() -> String {
    let profile = profile_name();
    if profile == "default" {
        return "margins".to_string();
    }
    let slug = profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!(
        "margins-{}",
        if slug.is_empty() { "profile" } else { &slug }
    )
}

fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("~"))
            .to_string_lossy()
            .to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

fn default_vault_path_string() -> String {
    "~/Documents/margins".to_string()
}

fn default_people_folder() -> String {
    "people".to_string()
}

fn default_inbox_folder() -> String {
    "meetings".to_string()
}

fn default_project_readiness() -> String {
    "needs_setup".to_string()
}

fn project_id_from_path(path: &str) -> String {
    let mut out = String::new();
    for ch in path.trim_start_matches("~/").chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 42 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

fn project_name_from_path(path: &str) -> String {
    let clean = path.trim().trim_end_matches('/');
    let leaf = clean
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("Project");
    leaf.chars()
        .map(|ch| if ch == '-' || ch == '_' { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_project_source(path: &str, inbox_folder: &str, people_folder: &str) -> ProjectSource {
    ProjectSource {
        id: project_id_from_path(path),
        name: project_name_from_path(path),
        path: path.to_string(),
        inbox_folder: inbox_folder.to_string(),
        people_folder: if people_folder.trim().is_empty() {
            default_people_folder()
        } else {
            people_folder.to_string()
        },
        readiness: default_project_readiness(),
    }
}

trait NonEmptyString {
    fn or_else_nonempty(self, fallback: impl FnOnce() -> Option<String>) -> Option<String>;
}

impl NonEmptyString for String {
    fn or_else_nonempty(self, fallback: impl FnOnce() -> Option<String>) -> Option<String> {
        if self.trim().is_empty() {
            fallback()
        } else {
            Some(self)
        }
    }
}

/// Input to [`upsert_project`].
pub struct UpsertProject<'a> {
    /// Raw path string (tilde-expanded internally).
    pub path: &'a str,
    /// Override the project display name; derived from path leaf when absent.
    pub name: Option<&'a str>,
    /// Override the inbox sub-folder; defaults to `"meetings"` when absent.
    pub inbox_folder: Option<&'a str>,
    /// Override the people sub-folder; defaults to `"people"` when absent.
    pub people_folder: Option<&'a str>,
    /// Override the readiness tag; defaults to `"needs_setup"` when absent.
    pub readiness: Option<&'a str>,
    /// Preferred id for **new** entries.  Ignored when an existing entry already
    /// matches by id or canonical path (the stored id is kept in that case).
    pub id_hint: Option<&'a str>,
}

/// Outcome of a successful [`upsert_project`] call.
pub enum UpsertOutcome {
    /// A new project entry was appended to the settings store.
    Inserted(ResolvedProject),
    /// An existing entry (matched by id or canonical path) was updated in place.
    Updated(ResolvedProject),
}

impl UpsertOutcome {
    /// Borrow the [`ResolvedProject`] regardless of outcome variant.
    pub fn resolved(&self) -> &ResolvedProject {
        match self {
            UpsertOutcome::Inserted(r) | UpsertOutcome::Updated(r) => r,
        }
    }
}

/// Upsert a project entry in settings.json using a raw-Value patch so that
/// desktop-only keys (ai_*, calendar_*, audio device, templates, …) are
/// never dropped.  Sets the project as the active project.
///
/// This function validates that the path exists and is a directory, but does
/// **not** create `.margins/` — callers that need that directory (e.g.
/// [`add_project`]) are responsible for creating it.
pub fn upsert_project(input: &UpsertProject<'_>) -> Result<UpsertOutcome> {
    let expanded = expand_tilde(input.path.trim());
    let root = PathBuf::from(&expanded);
    if !root.exists() {
        bail!("Project path does not exist: {}", root.display());
    }
    if !root.is_dir() {
        bail!("Project path is not a directory: {}", root.display());
    }
    // Use canonical path so the stored path is stable across symlinks/relative refs.
    let canonical = root
        .canonicalize()
        .with_context(|| format!("Could not canonicalize {}", root.display()))?;
    let canonical_str = canonical.to_string_lossy().to_string();

    // Derive field values, applying defaults where the caller did not supply them.
    let derived_id = project_id_from_path(&canonical_str);
    let id = input
        .id_hint
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&derived_id);
    let proj_name = input
        .name
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| project_name_from_path(&canonical_str));
    let inbox = input
        .inbox_folder
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(default_inbox_folder);
    let people = input
        .people_folder
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(default_people_folder);
    let readiness = input
        .readiness
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(default_project_readiness);

    // Read (or bootstrap) settings.json as a raw Value so unknown keys are preserved.
    let settings_file = settings_path();
    let mut value: Value = if settings_file.exists() {
        let raw = std::fs::read_to_string(&settings_file).with_context(|| {
            format!(
                "Could not read Margins settings at {}",
                settings_file.display()
            )
        })?;
        serde_json::from_str(&raw).context("Margins settings are not valid JSON")?
    } else {
        serde_json::json!({
            "vault_path": canonical_str,
            "projects": [],
            "active_project_id": null,
            "inbox_folder": default_inbox_folder(),
            "people_folder": default_people_folder()
        })
    };

    let obj = value
        .as_object_mut()
        .context("Margins settings root is not a JSON object")?;

    let new_entry = serde_json::json!({
        "id": id,
        "name": proj_name,
        "path": canonical_str,
        "inbox_folder": inbox,
        "people_folder": people,
        "readiness": readiness
    });

    let projects_arr = obj
        .entry("projects")
        .or_insert_with(|| Value::Array(vec![]))
        .as_array_mut()
        .context("'projects' in settings.json is not an array")?;

    // Match by id, or by canonicalized stored path: the desktop app stores the
    // raw picker path (no canonicalize), so the same physical folder can carry
    // a different id — update that entry in place instead of appending a twin.
    let existing_idx = projects_arr.iter().position(|entry| {
        if entry.get("id").and_then(Value::as_str) == Some(id) {
            return true;
        }
        entry
            .get("path")
            .and_then(Value::as_str)
            .map(|p| PathBuf::from(expand_tilde(p)))
            .and_then(|p| p.canonicalize().ok())
            .map(|p| p == canonical)
            .unwrap_or(false)
    });

    let (active_id, was_updated) = if let Some(idx) = existing_idx {
        // Update mutable fields in place; preserve the stored id and path spelling
        // so the rest of the app's references continue to resolve.
        if let Some(entry_obj) = projects_arr[idx].as_object_mut() {
            entry_obj.insert("name".to_string(), Value::String(proj_name.clone()));
            entry_obj.insert("inbox_folder".to_string(), Value::String(inbox.clone()));
            entry_obj.insert("people_folder".to_string(), Value::String(people.clone()));
            entry_obj.insert("readiness".to_string(), Value::String(readiness.clone()));
        }
        let stored_id = projects_arr[idx]
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        (stored_id, true)
    } else {
        projects_arr.push(new_entry);
        (id.to_string(), false)
    };

    obj.insert(
        "active_project_id".to_string(),
        Value::String(active_id.clone()),
    );

    // Atomic write (tmp + rename) so the app's settings watcher never sees a
    // truncated file mid-write.
    std::fs::create_dir_all(
        settings_file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid Margins settings path"))?,
    )?;
    let tmp_file = settings_file.with_extension("json.tmp");
    std::fs::write(&tmp_file, serde_json::to_string_pretty(&value)?)
        .with_context(|| format!("Could not write {}", tmp_file.display()))?;
    std::fs::rename(&tmp_file, &settings_file).with_context(|| {
        format!(
            "Could not write Margins settings at {}",
            settings_file.display()
        )
    })?;

    let source = ProjectSource {
        id: active_id,
        name: proj_name,
        path: canonical_str,
        inbox_folder: inbox,
        people_folder: people,
        readiness,
    };
    let resolved = resolve_project_paths(source);
    if was_updated {
        Ok(UpsertOutcome::Updated(resolved))
    } else {
        Ok(UpsertOutcome::Inserted(resolved))
    }
}

/// Add (or update) a project in settings.json using a raw-Value patch so that
/// desktop-only keys (ai_*, calendar_*, audio device, templates, …) are
/// never dropped.  Creates `<root>/.margins/` and sets the project as active.
pub fn add_project(
    path: &str,
    name: Option<&str>,
    inbox_folder: Option<&str>,
) -> Result<ResolvedProject> {
    let input = UpsertProject {
        path,
        name,
        inbox_folder,
        people_folder: None,
        readiness: None,
        id_hint: None,
    };
    let outcome = upsert_project(&input)?;
    let resolved = outcome.resolved();

    // Create .margins/ under the canonical project root.
    let margins_dir = resolved.root_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir)
        .with_context(|| format!("Could not create {}", margins_dir.display()))?;

    Ok(match outcome {
        UpsertOutcome::Inserted(r) | UpsertOutcome::Updated(r) => r,
    })
}

/// Write the local setup skills into `<root>/.margins/skills/`.
/// The generic Enzyme skill is mirrored from enzyme-rust; the Margins wrapper
/// owns Desktop-specific project, binary, and destination language.
pub fn install_local_setup_skill(root: &Path) -> Result<PathBuf> {
    let skills_root = root.join(".margins").join("skills");
    let enzyme_destination = write_local_skill(
        &skills_root,
        "enzyme-workspace-setup",
        WORKSPACE_SETUP_SKILL,
    )?;
    write_local_skill(
        &skills_root,
        "margins-workspace-setup",
        MARGINS_WORKSPACE_SETUP_SKILL,
    )?;
    Ok(enzyme_destination)
}

fn write_local_skill(skills_root: &Path, name: &str, content: &str) -> Result<PathBuf> {
    let skill_dir = skills_root.join(name);
    std::fs::create_dir_all(&skill_dir)
        .with_context(|| format!("Could not create skill directory {}", skill_dir.display()))?;
    let destination = skill_dir.join("SKILL.md");
    std::fs::write(&destination, content)
        .with_context(|| format!("Could not write skill to {}", destination.display()))?;
    Ok(destination)
}

/// Run `enzyme -p <root> init`, searching PATH then /opt/homebrew/bin/enzyme
/// then ~/.local/bin/enzyme.  Returns an error if the binary is missing or
/// exits non-zero; never panics.
pub fn init_enzyme(root: &Path) -> Result<()> {
    let enzyme_bin = find_enzyme_binary().ok_or_else(|| {
        anyhow::anyhow!(
            "enzyme binary not found. Install it with `brew install enzyme` or ensure it is on PATH."
        )
    })?;
    let status = std::process::Command::new(&enzyme_bin)
        .arg("-p")
        .arg(root)
        .arg("init")
        .status()
        .with_context(|| {
            format!(
                "Failed to run {} -p {} init",
                enzyme_bin.display(),
                root.display()
            )
        })?;
    if !status.success() {
        bail!(
            "`{} -p {} init` exited with status {}",
            enzyme_bin.display(),
            root.display(),
            status
        );
    }
    Ok(())
}

fn find_enzyme_binary() -> Option<PathBuf> {
    // 1. PATH
    if let Ok(output) = std::process::Command::new("which").arg("enzyme").output() {
        if output.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim().to_string());
            if p.exists() {
                return Some(p);
            }
        }
    }
    // 2. Common fixed locations
    for candidate in &["/opt/homebrew/bin/enzyme", "/usr/local/bin/enzyme"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }
    // 3. ~/.local/bin/enzyme
    if let Some(home) = dirs::home_dir() {
        let p = home.join(".local/bin/enzyme");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

pub fn project_to_xml(project: &ResolvedProject, active: bool) -> String {
    let mut out = format!(
        "  <project id=\"{}\" active=\"{}\">\n",
        xml_escape_attr(&project.project.id),
        active
    );
    out.push_str(&format!(
        "    <name>{}</name>\n",
        xml_escape_text(&project.project.name)
    ));
    out.push_str(&format!(
        "    <path>{}</path>\n",
        xml_escape_text(&project.root_dir.to_string_lossy())
    ));
    out.push_str(&format!(
        "    <work_dir>{}</work_dir>\n",
        xml_escape_text(&project.work_dir.to_string_lossy())
    ));
    out.push_str(&format!(
        "    <inbox_dir>{}</inbox_dir>\n",
        xml_escape_text(&inbox_dir(project).to_string_lossy())
    ));
    out.push_str(&format!(
        "    <inbox_folder>{}</inbox_folder>\n",
        xml_escape_text(&project.project.inbox_folder)
    ));
    out.push_str("  </project>\n");
    out
}

fn xml_escape_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| *c == '\n' || *c == '\r' || *c == '\t' || !c.is_control())
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            _ => vec![c],
        })
        .collect()
}

pub fn xml_escape_attr(value: &str) -> String {
    xml_escape_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate MARGINS_PROFILE env var
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Returns the settings.json path for a given profile slug.
    fn settings_file_for_profile(profile: &str) -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(format!("margins-{profile}"))
            .join("settings.json")
    }

    /// Generate a profile name that is safe for profile_slug() (alphanumeric + dash only).
    fn unique_test_profile() -> String {
        // Use thread id as a stable-per-test but unique identifier
        let tid = std::thread::current().id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_micros();
        // profile_slug will lowercase and keep alphanumeric/dash/underscore
        format!("t{:?}-{}", tid, ts)
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }

    #[test]
    fn add_project_creates_entry_and_preserves_unknown_keys() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("myproject");
        std::fs::create_dir_all(&project_dir).unwrap();

        let profile = unique_test_profile();
        std::env::set_var("MARGINS_PROFILE", &profile);

        // Pre-seed a settings.json with an unknown desktop-only key
        let settings_file = settings_file_for_profile(&profile);
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();
        let seed = serde_json::json!({
            "ai_model": "x",
            "projects": [],
            "active_project_id": null,
            "vault_path": "",
            "inbox_folder": "meetings",
            "people_folder": "people"
        });
        std::fs::write(&settings_file, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        let resolved = add_project(&project_dir.to_string_lossy(), None, None).unwrap();

        // The new project should be active
        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
        assert_eq!(
            after["active_project_id"].as_str(),
            Some(resolved.project.id.as_str()),
            "active_project_id mismatch"
        );
        // Unknown key must still be present
        assert_eq!(
            after["ai_model"].as_str(),
            Some("x"),
            "desktop-only key 'ai_model' was dropped by add_project"
        );
        let projects = after["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);

        std::env::remove_var("MARGINS_PROFILE");
        let _ = std::fs::remove_dir_all(settings_file.parent().unwrap());
    }

    #[test]
    fn add_project_is_idempotent() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("idempotent");
        std::fs::create_dir_all(&project_dir).unwrap();

        let profile = unique_test_profile();
        std::env::set_var("MARGINS_PROFILE", &profile);

        let settings_file = settings_file_for_profile(&profile);
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();

        // First add
        add_project(&project_dir.to_string_lossy(), None, None).unwrap();
        // Second add with same path — must be idempotent
        add_project(&project_dir.to_string_lossy(), None, None).unwrap();

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
        let projects = after["projects"].as_array().unwrap();
        assert_eq!(
            projects.len(),
            1,
            "add_project should be idempotent (1 entry expected, got {})",
            projects.len()
        );

        std::env::remove_var("MARGINS_PROFILE");
        let _ = std::fs::remove_dir_all(settings_file.parent().unwrap());
    }

    #[test]
    fn add_project_rejects_nonexistent_path() {
        let _guard = ENV_LOCK.lock().unwrap();
        let profile = unique_test_profile();
        std::env::set_var("MARGINS_PROFILE", &profile);

        let result = add_project("/tmp/__margins_test_nonexistent_path_xyz__", None, None);
        assert!(result.is_err(), "Expected error for non-existent path");

        std::env::remove_var("MARGINS_PROFILE");
    }

    #[test]
    fn install_local_setup_skill_writes_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = install_local_setup_skill(tmp.path()).unwrap();
        assert!(dest.exists(), "SKILL.md was not written");
        let content = std::fs::read_to_string(&dest).unwrap();
        assert_eq!(
            content, WORKSPACE_SETUP_SKILL,
            "written enzyme-workspace-setup SKILL.md is not byte-identical to the bundled skill"
        );
        let margins_dest = tmp
            .path()
            .join(".margins")
            .join("skills")
            .join("margins-workspace-setup")
            .join("SKILL.md");
        let margins_content = std::fs::read_to_string(&margins_dest).unwrap();
        assert_eq!(
            margins_content, MARGINS_WORKSPACE_SETUP_SKILL,
            "written margins-workspace-setup SKILL.md is not byte-identical to the bundled skill"
        );
    }

    #[test]
    fn normalizes_project_work_dir_to_project_root() {
        let settings = ProjectSettings {
            vault_path: None,
            projects: vec![ProjectSource {
                id: "p".to_string(),
                name: "Project".to_string(),
                path: "~/vault".to_string(),
                inbox_folder: "meetings".to_string(),
                people_folder: String::new(),
                readiness: String::new(),
            }],
            active_project_id: Some("p".to_string()),
            inbox_folder: String::new(),
            people_folder: String::new(),
        };
        let project = resolve_project_paths(normalize_projects(&settings).remove(0));
        assert!(project.work_dir.ends_with("vault"));
    }

    #[test]
    fn defaults_missing_project_folder_to_meetings() {
        let raw = r#"{
            "vault_path": "/vault",
            "projects": [{
                "id": "vault",
                "name": "Vault",
                "path": "/vault"
            }],
            "active_project_id": "vault"
        }"#;
        let settings: ProjectSettings = serde_json::from_str(raw).unwrap();
        let projects = normalize_projects(&settings);

        assert_eq!(settings.inbox_folder, "meetings");
        assert_eq!(projects[0].inbox_folder, "meetings");
    }

    #[test]
    fn project_xml_preserves_cli_shape_and_escapes_fields() {
        let project = ResolvedProject {
            project: ProjectSource {
                id: "a&\"b".into(),
                name: "A < B".into(),
                path: "/tmp/a&b".into(),
                inbox_folder: "meetings>calls".into(),
                people_folder: "people".into(),
                readiness: "ready".into(),
            },
            root_dir: PathBuf::from("/tmp/a&b"),
            work_dir: PathBuf::from("/tmp/a&b"),
        };
        let xml = project_to_xml(&project, true);
        assert!(xml.starts_with("  <project id=\"a&amp;&quot;b\" active=\"true\">\n"));
        assert!(xml.contains("    <name>A &lt; B</name>\n"));
        assert!(xml.contains("    <path>/tmp/a&amp;b</path>\n"));
        assert!(xml.contains("    <inbox_folder>meetings&gt;calls</inbox_folder>\n"));
        assert!(xml.ends_with("  </project>\n"));
    }
}
