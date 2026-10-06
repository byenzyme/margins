//! The preset program Workspace setup starts from.
//!
//! Margins ships `margins-meetings.enzyme.in`, an Enzyme preset template.
//! Setup has the engine fill it (`enzyme compile --preset <path> --dry-run`,
//! in a throwaway engine home so the preview writes nothing) for the
//! Workspace's notes folder, then [`propose`] turns the filled program into the
//! desired Workspace view: folder readings whose folder the notes folder does
//! not have are dropped, and the rest is added to the current program. The
//! result goes through the ordinary `workspace plan` / `apply` review.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::workspace::{SourceRole, WorkspaceBinding, WorkspaceConfig, WorkspaceEntity};
use crate::workspace_program::{derive_view, home_folder_entity, WorkspaceProgram};

/// The preset setup uses.
pub const MEETINGS_PRESET: &str = "margins-meetings";
/// The shipped text of [`MEETINGS_PRESET`].
pub const MEETINGS_PRESET_TEXT: &str =
    include_str!("../resources/presets/margins-meetings.enzyme.in");

/// The desired Workspace a filled preset proposes, and what it kept.
#[derive(Debug, Clone, Serialize)]
pub struct PresetProposal {
    #[serde(skip)]
    pub desired: WorkspaceConfig,
    /// Preset readings in the desired program, as written there.
    pub readings: Vec<String>,
    /// Preset folder readings dropped because the notes folder has no such
    /// folder, or several that differ only in case.
    pub skipped_readings: Vec<String>,
    /// Why each skipped reading was dropped.
    pub skip_reasons: BTreeMap<String, String>,
    /// The Home folder Margins writes notes into (`"."` is the Home root).
    pub note_folder: String,
}

/// Add the filled preset `filled` to `current`.
///
/// Only additions are made, so running setup again proposes no change:
/// - each preset reading is added unless an equal entity is already read; a
///   folder reading is added only when the folder exists under Home (matched
///   case-insensitively and written with its on-disk name), else skipped;
/// - preset `leave out folders` entries are added;
/// - the preset's `learn questions automatically` is added to a program that
///   has none; an existing one, with its own `up to`, is kept;
/// - the preset note folder is used only by a program that has not been set
///   up yet: Home-root (`"."`) note folder, no readings, nothing left out (what
///   `workspace new` writes). Any other `"."` is a choice and is kept.
pub fn propose(current: &WorkspaceConfig, filled: &str) -> Result<PresetProposal> {
    let preset = WorkspaceProgram::parse(filled).context("the filled preset does not parse")?;
    let preset = derive_view(&preset, None, Default::default())?;
    let never_set_up =
        current.policy.entities.is_empty() && current.policy.excluded_folders.is_empty();
    let mut desired = current.clone();
    let home = home_path(&desired)?.to_path_buf();
    let mut readings = Vec::new();
    let mut skipped_readings = Vec::new();
    let mut skip_reasons = BTreeMap::new();

    for entity in &preset.policy.entities {
        for (entity_ref, options) in entity.entries() {
            let entity_ref = match folder_of(entity_ref) {
                Some(folder) => match find_home_folder(&home, folder)? {
                    HomeFolder::Found(actual) => home_folder_entity(
                        &desired,
                        &enzyme_spec::entity_selector("folder", &slash_path(&actual)),
                    ),
                    HomeFolder::Missing => {
                        skipped_readings.push(entity_ref.to_string());
                        skip_reasons.insert(entity_ref.to_string(), "no such folder".to_string());
                        continue;
                    }
                    HomeFolder::Ambiguous(paths) => {
                        skipped_readings.push(entity_ref.to_string());
                        skip_reasons.insert(
                            entity_ref.to_string(),
                            format!(
                                "several folders differ only in case: {}",
                                paths.iter().map(|path| slash_path(path)).collect::<Vec<_>>().join(", ")
                            ),
                        );
                        continue;
                    }
                },
                None => entity_ref.to_string(),
            };
            readings.push(entity_ref.clone());
            let already_read = desired.policy.entities.iter().any(|existing| {
                existing
                    .entries()
                    .iter()
                    .any(|(existing, _)| existing.eq_ignore_ascii_case(&entity_ref))
            });
            if !already_read {
                desired.policy.entities.push(match options {
                    Some(options) => WorkspaceEntity::with_options(entity_ref, options.clone()),
                    None => WorkspaceEntity::simple(entity_ref),
                });
            }
        }
    }

    for folder in &preset.policy.excluded_folders {
        let folder = match find_home_folder(&home, folder)? {
            HomeFolder::Found(actual) => slash_path(&actual),
            HomeFolder::Missing | HomeFolder::Ambiguous(_) => folder.clone(),
        };
        if !desired
            .policy
            .excluded_folders
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&folder))
        {
            desired.policy.excluded_folders.push(folder);
        }
    }

    if desired.policy.automatic.is_none() {
        desired.policy.automatic = preset.policy.automatic;
    }

    let preset_note_folder = preset
        .bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                note_folder,
                ..
            } => note_folder.clone(),
            _ => None,
        });
    let note_folder = home_note_folder(&mut desired)?;
    if note_folder.is_none() && never_set_up {
        if let Some(folder) = preset_note_folder {
            let folder = match find_home_folder(&home, &slash_path(&folder))? {
                HomeFolder::Found(actual) => actual,
                HomeFolder::Missing | HomeFolder::Ambiguous(_) => folder,
            };
            *note_folder = Some(folder);
        }
    }
    let note_folder = note_folder
        .as_deref()
        .map(slash_path)
        .unwrap_or_else(|| ".".to_string());

    Ok(PresetProposal {
        desired,
        readings,
        skipped_readings,
        skip_reasons,
        note_folder,
    })
}

fn home_path(view: &WorkspaceConfig) -> Result<&Path> {
    view.bindings
        .values()
        .find_map(|binding| match binding {
            WorkspaceBinding::NativeMarkdown {
                path,
                role: SourceRole::Home,
                ..
            } => Some(path.as_path()),
            _ => None,
        })
        .context("Workspace has no Home notes source")
}

fn home_note_folder(view: &mut WorkspaceConfig) -> Result<&mut Option<PathBuf>> {
    view.bindings
        .values_mut()
        .find_map(|binding| match binding {
            WorkspaceBinding::NativeMarkdown {
                role: SourceRole::Home,
                note_folder,
                ..
            } => Some(note_folder),
            _ => None,
        })
        .context("Workspace has no Home notes source")
}

fn folder_of(entity_ref: &str) -> Option<&str> {
    let (kind, name) = enzyme_spec::split_entity(entity_ref.trim());
    kind.eq_ignore_ascii_case("folder").then_some(name)
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Where a Home-relative folder path points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeFolder {
    /// The folder, relative to Home, with its on-disk names.
    Found(PathBuf),
    Missing,
    /// Several folders differ from the request only in case: their paths.
    Ambiguous(Vec<PathBuf>),
}

/// Find the folder `relative` (`/`-separated, `"."` for the root) under
/// `home`. Each component matches case-insensitively (Unicode lowercase); an
/// exact match wins over other spellings.
pub fn find_home_folder(home: &Path, relative: &str) -> Result<HomeFolder> {
    let relative = relative.trim();
    if relative == "." {
        return Ok(if home.is_dir() {
            HomeFolder::Found(PathBuf::new())
        } else {
            HomeFolder::Missing
        });
    }
    let components = Path::new(relative).components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("folder path must be relative to the Workspace home: {relative:?}");
    }
    let mut found = PathBuf::new();
    for component in components {
        let wanted = component.as_os_str().to_string_lossy();
        let wanted_folded = wanted.to_lowercase();
        let current = home.join(&found);
        let entries = std::fs::read_dir(&current).with_context(|| {
            format!("could not inspect Workspace home folder {}", current.display())
        })?;
        let matches = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().to_lowercase() == wanted_folded)
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>();
        let next = match matches.as_slice() {
            [] => return Ok(HomeFolder::Missing),
            [only] => only.clone(),
            several => match several.iter().find(|name| name.to_string_lossy() == wanted) {
                Some(exact) => exact.clone(),
                None => {
                    let mut paths = several.iter().map(|name| found.join(name)).collect::<Vec<_>>();
                    paths.sort();
                    return Ok(HomeFolder::Ambiguous(paths));
                }
            },
        };
        found.push(next);
    }
    Ok(HomeFolder::Found(found))
}

/// [`find_home_folder`], with several matching spellings as an error.
pub fn resolve_home_folder(home: &Path, relative: &str) -> Result<Option<PathBuf>> {
    match find_home_folder(home, relative)? {
        HomeFolder::Found(path) => Ok(Some(path)),
        HomeFolder::Missing => Ok(None),
        HomeFolder::Ambiguous(_) => {
            bail!("folder path {relative:?} is ambiguous under the Workspace home")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::WorkspaceEntityOptions;

    fn filled(home: &Path) -> String {
        MEETINGS_PRESET_TEXT
            .replace("__WORKSPACE__", "\"w\"")
            .replace("__SOURCE__", &format!("{:?}", home.display().to_string()))
    }

    fn current(home: &Path) -> WorkspaceConfig {
        let text = format!(
            "workspace \"w\" {{\n  source margins-captures \"captures\" {{ path \"/state/captures\" }}\n  source markdown \"home\" {{ path {:?} }}\n  remember in folder \".\" create note\n}}\n",
            home.display().to_string()
        );
        derive_view(&WorkspaceProgram::parse(&text).unwrap(), None, Default::default()).unwrap()
    }

    fn entity_refs(view: &WorkspaceConfig) -> Vec<String> {
        view.policy
            .entities
            .iter()
            .flat_map(|entity| {
                entity
                    .entries()
                    .into_iter()
                    .map(|(entity_ref, _)| entity_ref.to_string())
            })
            .collect()
    }

    #[test]
    fn keeps_existing_folders_with_their_on_disk_names_and_drops_the_rest() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("meetings")).unwrap();
        std::fs::create_dir_all(temp.path().join("People")).unwrap();
        std::fs::create_dir_all(temp.path().join("templates")).unwrap();
        let proposal = propose(&current(temp.path()), &filled(temp.path())).unwrap();

        assert_eq!(entity_refs(&proposal.desired), ["folder:meetings", "folder:People"]);
        assert_eq!(proposal.readings, ["folder:meetings", "folder:People"]);
        assert_eq!(proposal.skipped_readings, ["folder:Projects"]);
        assert_eq!(proposal.note_folder, "meetings");
        assert_eq!(
            proposal.desired.policy.excluded_folders,
            ["templates", "Attachments"]
        );
        let people = &proposal.desired.policy.entities[1];
        assert_eq!(
            people.entries()[0].1,
            Some(&WorkspaceEntityOptions {
                profile: Some("relationships".into()),
                expandable: true,
                children: Vec::new(),
            })
        );
        assert!(proposal.desired.bindings.contains_key("captures"));
    }

    #[test]
    fn proposing_again_changes_nothing_and_keeps_user_choices() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Meetings")).unwrap();
        std::fs::create_dir_all(temp.path().join("Inbox")).unwrap();
        let first = propose(&current(temp.path()), &filled(temp.path())).unwrap();
        let second = propose(&first.desired, &filled(temp.path())).unwrap();
        assert_eq!(second.desired, first.desired);

        let mut edited = first.desired.clone();
        edited.policy.entities.push(WorkspaceEntity::simple("tag:client"));
        if let Some(WorkspaceBinding::NativeMarkdown { note_folder, .. }) =
            edited.bindings.get_mut("home")
        {
            *note_folder = Some("Inbox".into());
        }
        let again = propose(&edited, &filled(temp.path())).unwrap();
        assert_eq!(again.desired, edited);
        assert_eq!(again.note_folder, "Inbox");

        // Notes deliberately written to the Home root stay there.
        if let Some(WorkspaceBinding::NativeMarkdown { note_folder, .. }) =
            edited.bindings.get_mut("home")
        {
            *note_folder = None;
        }
        let root = propose(&edited, &filled(temp.path())).unwrap();
        assert_eq!(root.desired, edited);
        assert_eq!(root.note_folder, ".");
    }

    #[test]
    fn automatic_selection_is_added_once_and_an_existing_cap_is_kept() {
        use crate::workspace::WorkspaceAutomatic;
        let temp = tempfile::tempdir().unwrap();
        let first = propose(&current(temp.path()), &filled(temp.path())).unwrap();
        assert_eq!(first.desired.policy.automatic, Some(WorkspaceAutomatic::default()));

        // A program set up before the preset had the statement gains it.
        let mut older = first.desired.clone();
        older.policy.automatic = None;
        assert_eq!(propose(&older, &filled(temp.path())).unwrap().desired, first.desired);

        // The user's own `up to N` is kept.
        let mut capped = first.desired.clone();
        capped.policy.automatic = Some(WorkspaceAutomatic { up_to: Some(5) });
        assert_eq!(propose(&capped, &filled(temp.path())).unwrap().desired, capped);
    }

    #[test]
    fn a_reading_whose_folder_has_several_spellings_is_skipped_with_a_reason() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("people")).unwrap();
        std::fs::create_dir_all(temp.path().join("PEOPLE")).unwrap();
        if !temp.path().join("pEOPLE").exists() {
            let proposal = propose(&current(temp.path()), &filled(temp.path())).unwrap();
            assert!(proposal.skipped_readings.contains(&"folder:People".to_string()));
            assert_eq!(
                proposal.skip_reasons["folder:People"],
                "several folders differ only in case: PEOPLE, people"
            );
            assert_eq!(proposal.skip_reasons["folder:Projects"], "no such folder");
        }
    }

    #[test]
    fn an_empty_notes_folder_keeps_only_the_note_destination() {
        let temp = tempfile::tempdir().unwrap();
        let proposal = propose(&current(temp.path()), &filled(temp.path())).unwrap();
        assert!(proposal.desired.policy.entities.is_empty());
        assert_eq!(proposal.note_folder, "Meetings");
        assert_eq!(proposal.skipped_readings.len(), 3);
    }

    #[test]
    fn folder_lookup_is_case_insensitive_and_refuses_ambiguity() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Work/People")).unwrap();
        assert_eq!(
            resolve_home_folder(temp.path(), "work/people").unwrap(),
            Some(PathBuf::from("Work/People"))
        );
        assert_eq!(resolve_home_folder(temp.path(), "missing").unwrap(), None);
        assert!(resolve_home_folder(temp.path(), "../x").is_err());
        std::fs::create_dir_all(temp.path().join("Ärzte")).unwrap();
        assert_eq!(
            resolve_home_folder(temp.path(), "ärzte").unwrap(),
            Some(PathBuf::from("Ärzte"))
        );
        std::fs::create_dir_all(temp.path().join("work")).unwrap();
        // Case-insensitive file systems cannot hold both spellings.
        if temp.path().join("WORK").exists() {
            return;
        }
        assert_eq!(
            find_home_folder(temp.path(), "Work").unwrap(),
            HomeFolder::Found("Work".into())
        );
        assert_eq!(
            find_home_folder(temp.path(), "WORK").unwrap(),
            HomeFolder::Ambiguous(vec!["Work".into(), "work".into()])
        );
        assert!(resolve_home_folder(temp.path(), "WORK/people").is_err());
    }
}
