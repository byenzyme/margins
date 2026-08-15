use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(any(test, feature = "test-support"))]
use std::cell::RefCell;
use std::path::{Path, PathBuf};

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static TEST_SETTINGS_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Scoped settings-path injection for tests that exercise real registry I/O.
/// This is unavailable in production builds and affects only the current test
/// thread, so parallel tests cannot redirect one another's settings.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub struct TestSettingsPathOverride(Option<PathBuf>);

#[cfg(any(test, feature = "test-support"))]
impl Drop for TestSettingsPathOverride {
    fn drop(&mut self) {
        let previous = self.0.take();
        TEST_SETTINGS_PATH.with(|path| *path.borrow_mut() = previous);
    }
}

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn override_settings_path_for_test(path: PathBuf) -> TestSettingsPathOverride {
    let previous = TEST_SETTINGS_PATH.with(|current| current.replace(Some(path)));
    TestSettingsPathOverride(previous)
}

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

/// Walk UP from `cwd` looking for a directory that contains `.margins/` or
/// `.obsidian/`, exactly like git discovers `.git`. Returns the nearest vault
/// root (the folder that contains the marker), never the marker itself. This
/// lets a first Margins command launched from an Obsidian subfolder establish
/// its store at the real vault root. Purely read-only: it never creates
/// anything.
pub fn discover_vault_root(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    while let Some(current) = dir {
        if current.join(".margins").is_dir() || current.join(".obsidian").is_dir() {
            return Some(current.to_path_buf());
        }
        dir = current.parent();
    }
    None
}

/// Resolve the vault for a session command, git-style. The model is simply:
/// record in the current folder.
///
/// * An explicit `selector` resolves through the registry.
/// * Otherwise, walk up from `cwd` for a `.margins/` or `.obsidian/` folder —
///   the root wins over cwd so a session and its distilled note always belong
///   to the vault root and the corpus is never split by recording from a
///   subfolder.
/// * A one-off macOS launcher temp cwd instead uses the configured stable vault;
///   selecting that temp project explicitly remains authoritative.
/// * Otherwise, the current folder *is* the vault: return `cwd` as the root.
///   This never creates anything; recording commands create `.margins/` in cwd
///   and register it silently via [`register_vault_silently`].
pub fn resolve_vault(selector: Option<&str>, cwd: &Path) -> Result<ResolvedProject> {
    if let Some(selector) = selector.map(str::trim).filter(|s| !s.is_empty()) {
        return resolve_project(Some(selector));
    }

    let discovered = discover_vault_root(cwd);
    if is_ephemeral_launcher_dir(cwd) {
        if let Some(project) = configured_project_for_ephemeral_launcher() {
            return Ok(resolve_project_paths(project));
        }
    }

    let root = discovered.unwrap_or_else(|| cwd.to_path_buf());
    // Prefer folder config from a registered entry for this root so a known
    // vault honors a custom inbox/people layout; otherwise derive from the folder.
    let project = registered_project_for_root(&root).unwrap_or_else(|| {
        default_project_source(
            &root.to_string_lossy(),
            &default_inbox_folder(),
            &default_people_folder(),
        )
    });
    let project = ProjectSource {
        path: root.to_string_lossy().to_string(),
        ..project
    };
    Ok(resolve_project_paths(project))
}

/// Agent/plugin launchers sometimes materialize a process in a one-off
/// `tempfile` directory even though the surrounding session belongs to a
/// configured vault. Limit this exception to the launcher directory itself
/// (not arbitrary descendants of the system temp directory), so ordinary
/// record-in-current-folder behavior remains unchanged.
fn is_ephemeral_launcher_dir(path: &Path) -> bool {
    let path = canonicalize_for_compare(path);
    let temp = canonicalize_for_compare(&std::env::temp_dir());
    let Ok(relative) = path.strip_prefix(temp) else {
        return false;
    };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(name)) = components.next() else {
        return false;
    };
    components.next().is_none()
        && name
            .to_str()
            .is_some_and(|name| name.starts_with(".tmp") || name.starts_with("tmp."))
}

/// Prefer the explicitly active registered project for an ephemeral launcher.
/// If an older buggy capture already made the temporary directory active,
/// recover the stable legacy `vault_path` entry instead.
fn configured_project_for_ephemeral_launcher() -> Option<ProjectSource> {
    let settings = load_project_settings().ok()?;
    let projects = normalize_projects(&settings);
    let usable = |project: &&ProjectSource| {
        let root = PathBuf::from(expand_tilde(&project.path));
        root.is_dir() && !is_ephemeral_launcher_dir(&root)
    };

    settings
        .active_project_id
        .as_deref()
        .and_then(|id| {
            projects
                .iter()
                .find(|project| project.id == id && usable(project))
        })
        .or_else(|| {
            let configured = settings.vault_path.as_deref()?;
            let target = canonicalize_for_compare(&PathBuf::from(expand_tilde(configured)));
            projects.iter().find(|project| {
                usable(project)
                    && canonicalize_for_compare(&PathBuf::from(expand_tilde(&project.path)))
                        == target
            })
        })
        .cloned()
}

/// Find a registered project whose canonical root matches `root`, if any.
fn registered_project_for_root(root: &Path) -> Option<ProjectSource> {
    let settings = load_project_settings().ok()?;
    let target = canonicalize_for_compare(root);
    normalize_projects(&settings).into_iter().find(|project| {
        canonicalize_for_compare(&PathBuf::from(expand_tilde(&project.path))) == target
    })
}

/// Best-effort, silent registration of a vault root in settings.json so desktop
/// and `margins recent --all` can enumerate it. Called by recording commands on
/// first use. No folder names are written (folder layout is the skill's call);
/// desktop-configured projects with explicit folders are preserved by
/// [`upsert_project`]. Never announced; failures are swallowed — bookkeeping
/// must not break recording.
pub fn register_vault_silently(root: &Path) {
    let path = root.to_string_lossy();
    let _ = upsert_project_inner(
        &UpsertProject {
            path: &path,
            name: None,
            inbox_folder: None,
            people_folder: None,
            readiness: Some("ready"),
            id_hint: None,
        },
        false,
    );
}

/// Establish a Margins vault at or above `cwd`, git-style, and return its root.
///
/// The shared establisher behind `margins init` (establish-without-recording)
/// and the recording commands (`margins new`). It honors the nesting guard: if
/// an existing Margins or Obsidian vault is found by walking up from `cwd`, that
/// root is returned unchanged — a new `.margins/` is never nested inside it.
/// Otherwise `<cwd>/.margins/` is created and the vault is registered silently.
pub fn ensure_vault(cwd: &Path) -> Result<PathBuf> {
    if let Some(root) = discover_vault_root(cwd) {
        // Already inside a Margins or Obsidian vault — never nest. An
        // Obsidian-only root still needs its Margins store established here.
        std::fs::create_dir_all(root.join(".margins"))
            .with_context(|| format!("Could not create {}", root.join(".margins").display()))?;
        register_vault_silently(&root);
        return Ok(root);
    }
    std::fs::create_dir_all(cwd.join(".margins"))
        .with_context(|| format!("Could not create {}", cwd.join(".margins").display()))?;
    register_vault_silently(cwd);
    Ok(cwd.to_path_buf())
}

pub fn settings_path() -> PathBuf {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(path) = TEST_SETTINGS_PATH.with(|path| path.borrow().clone()) {
        return path;
    }
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
                "Unknown Margins vault '{selector}'. Pass the vault folder path with `--project`, or cd into the vault and run `margins init`."
            )
        })?
    } else {
        let active = settings.active_project_id.as_deref();
        active
            .and_then(|id| projects.iter().find(|project| project.id == id))
            .or_else(|| projects.first())
            .cloned()
            .context("No Margins vault found. Run `margins init` in the folder you want to use, or `margins new` to start recording.")?
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
            "Unknown Margins vault '{selector}'. Pass a folder path, or run `margins init` in the vault."
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
    upsert_project_inner(input, true)
}

fn upsert_project_inner(input: &UpsertProject<'_>, make_active: bool) -> Result<UpsertOutcome> {
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
    // Folders are OPTIONAL: a fresh vault leaves them unset so the distillation
    // skill can decide adaptively later. Only write a folder name when the
    // caller actually supplies one; on update, absent means "leave as-is".
    let inbox = input
        .inbox_folder
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let people = input
        .people_folder
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
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

    let mut new_entry_map = serde_json::Map::new();
    new_entry_map.insert("id".to_string(), Value::String(id.to_string()));
    new_entry_map.insert("name".to_string(), Value::String(proj_name.clone()));
    new_entry_map.insert("path".to_string(), Value::String(canonical_str.clone()));
    if let Some(inbox) = &inbox {
        new_entry_map.insert("inbox_folder".to_string(), Value::String(inbox.clone()));
    }
    if let Some(people) = &people {
        new_entry_map.insert("people_folder".to_string(), Value::String(people.clone()));
    }
    new_entry_map.insert("readiness".to_string(), Value::String(readiness.clone()));
    let new_entry = Value::Object(new_entry_map);

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

    // Folder values used to build the returned ResolvedProject: prefer what the
    // caller supplied, else the already-stored value, else the resolve-time
    // default. This never *writes* a default folder name into the registration.
    let mut stored_inbox: Option<String> = None;
    let mut stored_people: Option<String> = None;
    let (active_id, was_updated) = if let Some(idx) = existing_idx {
        stored_inbox = projects_arr[idx]
            .get("inbox_folder")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);
        stored_people = projects_arr[idx]
            .get("people_folder")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);
        // Update mutable fields in place; preserve the stored id and path spelling
        // so the rest of the app's references continue to resolve. Folder names
        // are only overwritten when the caller supplies one.
        if let Some(entry_obj) = projects_arr[idx].as_object_mut() {
            entry_obj.insert("name".to_string(), Value::String(proj_name.clone()));
            if let Some(inbox) = &inbox {
                entry_obj.insert("inbox_folder".to_string(), Value::String(inbox.clone()));
            }
            if let Some(people) = &people {
                entry_obj.insert("people_folder".to_string(), Value::String(people.clone()));
            }
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

    if make_active {
        obj.insert(
            "active_project_id".to_string(),
            Value::String(active_id.clone()),
        );
    }

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
        inbox_folder: inbox.or(stored_inbox).unwrap_or_else(default_inbox_folder),
        people_folder: people
            .or(stored_people)
            .unwrap_or_else(default_people_folder),
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

    struct ScopedTestSettings {
        path: PathBuf,
        _override: TestSettingsPathOverride,
        _dir: tempfile::TempDir,
    }

    impl ScopedTestSettings {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("settings.json");
            let path_override = override_settings_path_for_test(path.clone());
            Self {
                path,
                _override: path_override,
                _dir: dir,
            }
        }
    }

    #[test]
    fn ensure_vault_establishes_without_installing_workspace_setup_skill() {
        let _settings = ScopedTestSettings::new();
        let tmp = tempfile::tempdir().unwrap();

        let root = ensure_vault(tmp.path()).unwrap();

        assert_eq!(root, tmp.path());
        assert!(tmp.path().join(".margins").is_dir());
        assert!(!tmp.path().join(".margins/skills").exists());
    }

    #[test]
    fn add_project_creates_entry_and_preserves_unknown_keys() {
        let settings = ScopedTestSettings::new();
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("myproject");
        std::fs::create_dir_all(&project_dir).unwrap();

        // Pre-seed a settings.json with an unknown desktop-only key
        let settings_file = &settings.path;
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
    }

    #[test]
    fn add_project_is_idempotent() {
        let settings = ScopedTestSettings::new();
        let tmp = tempfile::tempdir().unwrap();
        let project_dir = tmp.path().join("idempotent");
        std::fs::create_dir_all(&project_dir).unwrap();

        let settings_file = &settings.path;
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
    }

    #[test]
    fn add_project_rejects_nonexistent_path() {
        let result = add_project("/tmp/__margins_test_nonexistent_path_xyz__", None, None);
        assert!(result.is_err(), "Expected error for non-existent path");
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

    #[test]
    fn discover_vault_root_walks_up_to_dot_margins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("vault");
        let nested = root.join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(root.join(".margins")).unwrap();

        let found = discover_vault_root(&nested).expect("should discover vault by walking up");
        assert_eq!(
            found.canonicalize().unwrap(),
            root.canonicalize().unwrap(),
            "discovery must return the folder containing .margins, not a child"
        );
    }

    #[test]
    fn discover_vault_root_walks_up_to_obsidian_root_before_margins_is_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("obsidian");
        let nested = root.join("inbox").join("calls");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();

        let found = discover_vault_root(&nested).expect("should discover the Obsidian root");
        assert_eq!(found.canonicalize().unwrap(), root.canonicalize().unwrap());
    }

    #[test]
    fn ensure_vault_from_obsidian_subfolder_establishes_margins_at_root() {
        let _settings = ScopedTestSettings::new();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("obsidian");
        let inbox = root.join("inbox");
        std::fs::create_dir_all(&inbox).unwrap();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();

        let established = ensure_vault(&inbox).unwrap();

        assert_eq!(
            established.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
        assert!(root.join(".margins").is_dir());
        assert!(!inbox.join(".margins").exists());
    }

    #[test]
    fn discover_vault_root_returns_none_without_dot_margins() {
        let tmp = tempfile::tempdir().unwrap();
        let nested = tmp.path().join("x").join("y");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(
            discover_vault_root(&nested).is_none(),
            "no .margins/ anywhere up the tree must resolve to None"
        );
    }

    #[test]
    fn resolve_vault_uses_cwd_when_no_vault_and_creates_nothing() {
        let _settings = ScopedTestSettings::new();

        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("plain");
        std::fs::create_dir_all(&cwd).unwrap();

        let resolved = resolve_vault(None, &cwd).unwrap();
        assert_eq!(
            resolved.root_dir.canonicalize().unwrap(),
            cwd.canonicalize().unwrap(),
            "with no vault, the current folder is the vault root"
        );
        assert!(
            !cwd.join(".margins").exists(),
            "resolve_vault must never create .margins/ itself"
        );
    }

    #[test]
    fn resolve_vault_walks_up_to_root_from_subfolder() {
        let _settings = ScopedTestSettings::new();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("vault");
        let sub = root.join("meetings").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(root.join(".margins")).unwrap();

        // Recording from a subfolder must resolve to the vault root, not the
        // subfolder, so the corpus is never split.
        let resolved = resolve_vault(None, &sub).unwrap();
        assert_eq!(
            resolved.root_dir.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_vault_prefers_registered_custom_folders() {
        let settings = ScopedTestSettings::new();
        let settings_file = &settings.path;
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("myvault");
        std::fs::create_dir_all(root.join(".margins")).unwrap();
        // A desktop-configured project with an explicit custom inbox folder.
        upsert_project(&UpsertProject {
            path: &root.to_string_lossy(),
            name: None,
            inbox_folder: Some("calls"),
            people_folder: None,
            readiness: Some("ready"),
            id_hint: None,
        })
        .unwrap();

        let resolved = resolve_vault(None, &root.join("sub")).unwrap();
        assert_eq!(resolved.project.inbox_folder, "calls");
        assert_eq!(
            resolved.root_dir.canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_vault_uses_configured_vault_for_ephemeral_launcher_cwd() {
        let settings = ScopedTestSettings::new();
        let settings_file = &settings.path;
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();

        let stable_parent = tempfile::tempdir().unwrap();
        let stable = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(stable.join(".margins")).unwrap();
        let launcher = tempfile::Builder::new()
            .prefix(".tmp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let stable_path = stable.to_string_lossy();
        let stable_id = "stable-vault".to_string();
        let launcher_path = launcher.path().to_string_lossy();
        let launcher_id = "ephemeral-launcher".to_string();
        let seed = serde_json::json!({
            "vault_path": stable_path,
            // Reproduce the damaged state from the old silent registration:
            // the temp project became active while vault_path stayed stable.
            "active_project_id": launcher_id,
            "projects": [
                { "id": stable_id, "name": "Obsidian", "path": stable_path, "readiness": "ready" },
                { "id": launcher_id, "name": "Launcher", "path": launcher_path, "readiness": "ready" }
            ]
        });
        std::fs::write(&settings_file, serde_json::to_string_pretty(&seed).unwrap()).unwrap();

        let resolved = resolve_vault(None, launcher.path()).unwrap();
        assert_eq!(
            resolved.root_dir.canonicalize().unwrap(),
            stable.canonicalize().unwrap()
        );

        // An explicit selector remains authoritative, including for recovery
        // of sessions captured into an old temporary vault.
        let explicit = resolve_vault(Some(&launcher_id), launcher.path()).unwrap();
        assert_eq!(
            explicit.root_dir.canonicalize().unwrap(),
            launcher.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn resolve_vault_keeps_normal_unregistered_cwd_semantics() {
        let _settings = ScopedTestSettings::new();

        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("ordinary-project");
        std::fs::create_dir_all(&cwd).unwrap();
        let resolved = resolve_vault(None, &cwd).unwrap();
        assert_eq!(
            resolved.root_dir.canonicalize().unwrap(),
            cwd.canonicalize().unwrap()
        );
    }

    #[test]
    fn register_vault_silently_registers_without_folder_names() {
        let settings = ScopedTestSettings::new();
        let settings_file = &settings.path;
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join(".margins")).unwrap();

        register_vault_silently(&root);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
        let projects = after["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "silent registration must add the vault");
        let entry = &projects[0];
        assert!(
            entry.get("inbox_folder").is_none() && entry.get("people_folder").is_none(),
            "silent registration must not write folder names"
        );
    }

    #[test]
    fn register_vault_silently_preserves_desktop_configured_folders() {
        let settings = ScopedTestSettings::new();
        let settings_file = &settings.path;
        std::fs::create_dir_all(settings_file.parent().unwrap()).unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("vault");
        std::fs::create_dir_all(root.join(".margins")).unwrap();
        // Desktop registered explicit folders first.
        upsert_project(&UpsertProject {
            path: &root.to_string_lossy(),
            name: None,
            inbox_folder: Some("calls"),
            people_folder: Some("contacts"),
            readiness: Some("ready"),
            id_hint: None,
        })
        .unwrap();
        let active_before = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(&settings_file).unwrap(),
        )
        .unwrap()["active_project_id"]
            .clone();

        let other = tmp.path().join("other-vault");
        std::fs::create_dir_all(other.join(".margins")).unwrap();

        // A later `margins new` silently re-registers — must not clobber folders.
        register_vault_silently(&root);
        // Registering a capture location is catalog bookkeeping, not a user
        // request to switch the configured active project.
        register_vault_silently(&other);

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&settings_file).unwrap()).unwrap();
        let project = &after["projects"].as_array().unwrap()[0];
        assert_eq!(project["inbox_folder"].as_str(), Some("calls"));
        assert_eq!(project["people_folder"].as_str(), Some("contacts"));
        assert_eq!(after["active_project_id"], active_before);
    }
}
