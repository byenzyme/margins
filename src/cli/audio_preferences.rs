use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
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
    let path = home.join("config.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
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
    fs::create_dir_all(home).with_context(|| format!("creating {}", home.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .open(home.join("config.lock"))?;
    lock.lock_exclusive()?;
    let path = home.join("config.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut document = if raw.trim().is_empty() {
        DocumentMut::new()
    } else {
        raw.parse::<DocumentMut>()
            .with_context(|| format!("parsing {}", path.display()))?
    };
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
    let mut temporary = tempfile::NamedTempFile::new_in(home)?;
    temporary.write_all(document.to_string().as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
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

    #[test]
    fn choice_persists_without_erasing_other_machine_settings() {
        let home = tempfile::tempdir().unwrap();
        fs::write(
            home.path().join("config.toml"),
            "# keep me\n[llm]\nmode = 'local'\n",
        )
        .unwrap();
        let choice = InputPreference {
            name: "Yeti Stereo Microphone".into(),
            uid: Some("Yeti-uid".into()),
        };
        save_at(home.path(), &choice).unwrap();
        assert_eq!(load_at(home.path()).unwrap(), Some(choice));
        let raw = fs::read_to_string(home.path().join("config.toml")).unwrap();
        assert!(raw.contains("# keep me"));
        assert!(raw.contains("mode = 'local'"));
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
