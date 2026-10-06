use anyhow::{Context, Result};
use margins_workflows::machine_config;
use std::path::Path;
use toml_edit::{value, DocumentMut, Item, Table};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct InputPreference {
    pub name: String,
    pub uid: Option<String>,
}

impl InputPreference {
    pub fn from_selected(selected: &crate::recorder::SelectedInputDevice) -> Self {
        Self {
            name: selected.name.clone(),
            uid: selected.uid.clone(),
        }
    }
}

pub(super) fn load() -> Result<Option<InputPreference>> {
    let home = margins_workflows::workspace::margins_home()?;
    load_at(&home)
}

fn load_at(home: &Path) -> Result<Option<InputPreference>> {
    let path = machine_config::machine_config_path(home);
    let Some(raw) = machine_config::read_machine_config_text(home)? else {
        return Ok(None);
    };
    let document = raw
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", path.display()))?;
    let Some(audio) = document.get("audio").and_then(Item::as_table) else {
        return Ok(None);
    };
    let Some(name) = audio.get("input_name").and_then(Item::as_str) else {
        return Ok(None);
    };
    if name.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(InputPreference {
        name: name.to_owned(),
        uid: audio
            .get("input_uid")
            .and_then(Item::as_str)
            .filter(|uid| !uid.is_empty())
            .map(str::to_owned),
    }))
}

pub(super) fn save(selected: &crate::recorder::SelectedInputDevice) -> Result<()> {
    let home = margins_workflows::workspace::margins_home()?;
    save_at(&home, &InputPreference::from_selected(selected))
}

pub(super) fn remember_selection(
    app: &mut crate::app::App,
    selected: &crate::recorder::SelectedInputDevice,
) {
    app.current_mic_name = selected.name.clone();
    app.current_mic_uid = selected.uid.clone();
    app.preferred_mic_name = Some(selected.name.clone());
    app.preferred_mic_uid = selected.uid.clone();
    if let Err(error) = save(selected) {
        app.message = Some(format!("Mic preference not saved: {error}"));
    }
}

fn save_at(home: &Path, preference: &InputPreference) -> Result<()> {
    machine_config::update_machine_document(home, |document| {
        if document.get("audio").is_none() {
            document["audio"] = Item::Table(Table::new());
        }
        let audio = document["audio"]
            .as_table_mut()
            .context("machine audio config must be a table")?;
        audio["input_name"] = value(&preference.name);
        if let Some(uid) = &preference.uid {
            audio["input_uid"] = value(uid);
        } else {
            audio.remove("input_uid");
        }
        Ok(())
    })
}

/// A saved device may still appear in the device list while refusing an open.
/// Only that automatic choice falls back; an explicit picker choice reports its
/// failure so the user can choose another input.
pub(super) fn open_with_saved_fallback<S, T>(
    saved: Option<&InputPreference>,
    selected: Option<&S>,
    mut open: impl FnMut(Option<&S>) -> Result<T>,
) -> Result<(T, bool, Option<String>)> {
    match open(selected) {
        Ok(value) => Ok((value, false, None)),
        Err(error) if saved.is_some() && selected.is_some() => {
            let name = &saved.expect("checked above").name;
            let note = format!(
                "Saved mic '{name}' failed to open ({error:#}); using system default. ^D to choose input"
            );
            crate::cli_log::event("capture_saved_mic_fallback", note.clone());
            let value = open(None).with_context(|| format!("{note}; default input also failed"))?;
            Ok((value, true, Some(note)))
        }
        Err(error) => Err(error),
    }
}

pub(super) fn preferred_index(
    preference: &InputPreference,
    names: &[String],
    uids: &[Option<String>],
) -> Option<usize> {
    if let Some(uid) = &preference.uid {
        uids.iter()
            .position(|candidate| candidate.as_ref() == Some(uid))
    } else {
        names.iter().position(|name| name == &preference.name)
    }
}

fn unavailable_note(preference: &InputPreference) -> String {
    format!(
        "Saved mic '{}' is unavailable; using system default. ^D to choose input",
        preference.name
    )
}

pub(super) fn resolve(
    preference: Option<&InputPreference>,
) -> Result<(Option<crate::recorder::SelectedInputDevice>, Option<String>)> {
    let Some(preference) = preference else {
        return Ok((None, None));
    };
    let devices = crate::recorder::list_input_devices();
    let names = devices
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    let uids = crate::recorder::input_device_uid_snapshot(&names);
    let Some(index) = preferred_index(&preference, &names, &uids) else {
        return Ok((None, Some(unavailable_note(preference))));
    };
    match crate::recorder::selected_input_device(&names, &uids, index) {
        Ok(selected) => Ok((Some(selected), None)),
        Err(error) => Ok((
            None,
            Some(format!(
                "Saved mic '{}' could not open ({error}); using system default. ^D to choose input",
                preference.name
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn choice_persists_without_erasing_other_machine_settings() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "# keep me\n[workspace]\n# and me\ndefault = \"notes\"\n\n# engine\n[llm]\nmode = 'local'\n",
        )
        .unwrap();
        let choice = InputPreference {
            name: "Yeti Stereo Microphone".into(),
            uid: Some("Yeti-uid".into()),
        };
        save_at(home.path(), &choice).unwrap();
        assert_eq!(load_at(home.path()).unwrap(), Some(choice.clone()));
        // The legacy machine file migrated: host preferences stay together
        // with their comments, the generator moved to the engine settings
        // program, and its comment stays in the retired original.
        let raw = fs::read_to_string(home.path().join("margins.toml")).unwrap();
        assert!(raw.contains("# keep me\n[workspace]"), "{raw}");
        assert!(raw.contains("# and me\ndefault = \"notes\""), "{raw}");
        assert!(!raw.contains("# engine"), "{raw}");
        assert!(fs::read_to_string(home.path().join("config.toml.migrated"))
            .unwrap()
            .contains("# engine"));
        assert!(raw.contains("input_name"));
        assert!(fs::read_to_string(home.path().join("configs/settings.enzyme"))
            .unwrap()
            .contains("generation local"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.path().join("margins.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600,
                "machine config can name accounts, so it is owner-only"
            );
            fs::set_permissions(
                home.path().join("margins.toml"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            save_at(home.path(), &choice).unwrap();
            assert_eq!(
                fs::metadata(home.path().join("margins.toml"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn listed_saved_device_that_fails_to_open_uses_default_without_persisting() {
        let home = tempfile::tempdir().unwrap();
        let saved = InputPreference {
            name: "Busy mic".into(),
            uid: Some("busy-uid".into()),
        };
        let mut attempts = Vec::new();
        let (opened, fell_back, note) =
            open_with_saved_fallback(Some(&saved), Some(&"busy-uid"), |uid| {
                attempts.push(uid.copied());
                if uid.is_some() {
                    anyhow::bail!("device busy")
                } else {
                    Ok("default")
                }
            })
            .unwrap();
        assert_eq!(opened, "default");
        assert!(fell_back);
        assert!(note.unwrap().contains("device busy"));
        assert_eq!(attempts, vec![Some("busy-uid"), None]);
        assert_eq!(load_at(home.path()).unwrap(), None);
    }

    #[test]
    fn saved_uid_survives_reordering_and_missing_uid_falls_back() {
        let choice = InputPreference {
            name: "Yeti Stereo Microphone".into(),
            uid: Some("Yeti-uid".into()),
        };
        let names = vec!["USB Digital Audio".into(), choice.name.clone()];
        assert_eq!(
            preferred_index(
                &choice,
                &names,
                &[Some("USB-uid".into()), Some("Yeti-uid".into())]
            ),
            Some(1)
        );
        assert_eq!(
            preferred_index(&choice, &names, &[Some("USB-uid".into()), None]),
            None,
            "a missing saved UID must not silently choose another input by name"
        );
        assert!(unavailable_note(&choice).contains("using system default"));
    }
}
