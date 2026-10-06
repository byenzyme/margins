//! Machine configuration of a Margins home.
//!
//! The Margins home is an Enzyme home (`ENZYME_HOME=$MARGINS_HOME`). Host
//! preferences live in `margins.toml` (`[workspace] default`,
//! `[workspace.names]`, `[retention]`, and the CLI's own small preferences).
//! Engine settings live in the Enzyme program `configs/settings.enzyme`
//! (`settings { generation …; model "…"; updates disabled }`), which the engine
//! reads as part of its `configs/` directory.
//!
//! A root `config.toml` is Enzyme's legacy machine file. One left by an earlier
//! Margins is migrated automatically, under the machine lock, the first time
//! any reader or writer touches machine configuration: it is validated before
//! anything is written, `[llm] mode`/`local_model` move to `settings.enzyme`,
//! every other key moves to `margins.toml`, and the original is kept as
//! `config.toml.migrated`. Re-running is idempotent.

use anyhow::{bail, Context, Result};
use fs4::fs_std::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, TableLike};

use crate::workspace::{atomic_write, sync_parent_directory, CONFIGS_DIR};

/// Host preferences of a Margins home.
pub const MACHINE_CONFIG: &str = "margins.toml";
/// Enzyme's legacy machine file; Margins' machine config before `margins.toml`.
pub const LEGACY_MACHINE_CONFIG: &str = "config.toml";
pub const LEGACY_MACHINE_CONFIG_MIGRATED: &str = "config.toml.migrated";
/// Engine settings program under `configs/`.
pub const SETTINGS_PROGRAM: &str = "settings.enzyme";
/// Generation modes Margins selects; the engine's `settings { generation … }`.
pub const GENERATION_MODES: [&str; 3] = ["auto", "local", "hosted"];
const MACHINE_LOCK: &str = "config.lock";

pub fn machine_config_path(margins_home: &Path) -> PathBuf {
    margins_home.join(MACHINE_CONFIG)
}

pub fn settings_program_path(margins_home: &Path) -> PathBuf {
    margins_home.join(CONFIGS_DIR).join(SETTINGS_PROGRAM)
}

/// Exclusive machine configuration lock. Not re-entrant: code holding it must
/// use [`migrate_locked`] and the `*_locked` readers, never [`ensure_migrated`].
pub struct MachineLock {
    _file: File,
}

pub fn lock_machine(margins_home: &Path) -> Result<MachineLock> {
    std::fs::create_dir_all(margins_home)
        .with_context(|| format!("creating {}", margins_home.display()))?;
    let path = margins_home.join(MACHINE_LOCK);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok(MachineLock { _file: file })
}

/// Migrate a legacy root `config.toml`, if one is present. Takes the machine
/// lock only when there is something to migrate.
pub fn ensure_migrated(margins_home: &Path) -> Result<()> {
    if !margins_home.join(LEGACY_MACHINE_CONFIG).exists() {
        return Ok(());
    }
    let _lock = lock_machine(margins_home)?;
    migrate_locked(margins_home)
}

/// Migrate a legacy root `config.toml`; the caller holds the machine lock.
///
/// Values in the legacy file win over `margins.toml`/`settings.enzyme`: a
/// legacy file that is still present was written after (or instead of) the
/// last migration, for example by an earlier Margins.
pub fn migrate_locked(margins_home: &Path) -> Result<()> {
    let legacy_path = margins_home.join(LEGACY_MACHINE_CONFIG);
    let raw = match std::fs::read_to_string(&legacy_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", legacy_path.display()))
        }
    };
    let cannot_migrate = |error: anyhow::Error| {
        anyhow::anyhow!(
            "cannot migrate machine config {}: {error:#}; fix the file and run again (it is left unchanged)",
            legacy_path.display()
        )
    };
    let mut legacy = parse_document(&raw, &legacy_path).map_err(cannot_migrate)?;
    let (generation, model) = take_engine_settings(&mut legacy).map_err(cannot_migrate)?;

    let config_path = machine_config_path(margins_home);
    let mut config = read_document(&config_path)?.unwrap_or_default();
    merge_tables(config.as_table_mut(), legacy.as_table());

    let settings_path = settings_program_path(margins_home);
    let mut settings = read_settings_program(&settings_path)?.unwrap_or_default();
    if generation.is_some() {
        settings.settings.generation = generation;
    }
    if model.is_some() {
        settings.settings.local_model = model;
    }
    settings.settings.updates.get_or_insert(false);
    let settings_text = enzyme_spec::render_program(&settings);
    enzyme_spec::parse(&settings_text)
        .with_context(|| format!("rendering {}", settings_path.display()))?;

    // Everything is validated; a crash between these writes leaves the legacy
    // file in place, so the migration simply runs again.
    // A new margins.toml takes the legacy file's mode; an existing one keeps
    // its own, so a migration never loosens permissions.
    let permissions = match std::fs::metadata(&config_path) {
        Ok(existing) => existing.permissions(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => std::fs::metadata(&legacy_path)
            .with_context(|| format!("reading {}", legacy_path.display()))?
            .permissions(),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", config_path.display()))
        }
    };
    atomic_write(&config_path, config.to_string().as_bytes())?;
    std::fs::set_permissions(&config_path, permissions)
        .with_context(|| format!("setting permissions of {}", config_path.display()))?;
    atomic_write(&settings_path, settings_text.as_bytes())?;
    let backup = first_free(margins_home, LEGACY_MACHINE_CONFIG_MIGRATED);
    std::fs::rename(&legacy_path, &backup)
        .with_context(|| format!("retiring {}", legacy_path.display()))?;
    sync_parent_directory(&backup)?;
    log::info!(
        "migrated machine config {} to {} and {}",
        legacy_path.display(),
        config_path.display(),
        settings_path.display()
    );
    Ok(())
}

/// Remove `[llm] mode` and `[llm] local_model` from a legacy document,
/// validated, leaving any other `[llm]` keys in place.
fn take_engine_settings(document: &mut DocumentMut) -> Result<(Option<String>, Option<String>)> {
    let Some(llm) = document.get_mut("llm") else {
        return Ok((None, None));
    };
    let table = llm
        .as_table_like_mut()
        .context("machine config [llm] must be a table")?;
    let generation = match table.get("mode") {
        None => None,
        Some(item) => {
            let mode = item.as_str().context("[llm] mode must be a string")?;
            if !GENERATION_MODES.contains(&mode) {
                bail!("unsupported [llm] mode '{mode}'");
            }
            Some(mode.to_string())
        }
    };
    let model = match table.get("local_model") {
        None => None,
        Some(item) => {
            let model = item.as_str().context("[llm] local_model must be a string")?;
            Some(model.to_string())
        }
    };
    table.remove("mode");
    table.remove("local_model");
    if table.is_empty() {
        document.remove("llm");
    }
    Ok((generation, model))
}

/// Merge `from` into `into`, table by table; leaf values in `from` win.
fn merge_tables(into: &mut dyn TableLike, from: &dyn TableLike) {
    for (key, item) in from.iter() {
        match (into.get_mut(key), item.as_table_like()) {
            (Some(existing), Some(incoming)) if existing.is_table_like() => {
                merge_tables(existing.as_table_like_mut().expect("table-like"), incoming);
            }
            _ => {
                into.insert(key, item.clone());
            }
        }
    }
}

fn first_free(directory: &Path, name: &str) -> PathBuf {
    let first = directory.join(name);
    if !first.exists() {
        return first;
    }
    (1u32..)
        .map(|n| directory.join(format!("{name}.{n}")))
        .find(|path| !path.exists())
        .expect("a free retirement name")
}

fn parse_document(raw: &str, path: &Path) -> Result<DocumentMut> {
    if raw.trim().is_empty() {
        return Ok(DocumentMut::new());
    }
    raw.parse::<DocumentMut>()
        .with_context(|| format!("invalid machine config {}", path.display()))
}

fn read_document(path: &Path) -> Result<Option<DocumentMut>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => parse_document(&raw, path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// The settings program, which may define only `settings { … }`.
fn read_settings_program(path: &Path) -> Result<Option<enzyme_spec::Program>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let program = enzyme_spec::parse(&text)
        .with_context(|| format!("invalid settings program {}", path.display()))?;
    if let Some(workspace) = program.workspaces.first() {
        bail!(
            "{} declares Workspace '{}' from when that was a legal id; its notes and state are kept. \
             Rename it with `margins workspace rename {} <new-id>`",
            path.display(),
            workspace.name,
            workspace.name
        );
    }
    if !program.workspaces.is_empty()
        || !program.vaults.is_empty()
        || !program.profiles.is_empty()
        || program.learning != enzyme_spec::Learning::default()
        || program.retrieval.is_some()
    {
        bail!("{} may only define settings", path.display());
    }
    Ok(Some(program))
}

/// The raw text of `margins.toml` after migration, or `None` when absent.
pub fn read_machine_config_text(margins_home: &Path) -> Result<Option<String>> {
    ensure_migrated(margins_home)?;
    read_machine_config_text_locked(margins_home)
}

/// [`read_machine_config_text`] for a caller holding the machine lock (after
/// [`migrate_locked`]).
pub fn read_machine_config_text_locked(margins_home: &Path) -> Result<Option<String>> {
    let path = machine_config_path(margins_home);
    match std::fs::read_to_string(&path) {
        Ok(raw) => Ok(Some(raw)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Read-modify-write `margins.toml` under the machine lock, preserving
/// formatting and comments.
pub fn update_machine_document(
    margins_home: &Path,
    mutate: impl FnOnce(&mut DocumentMut) -> Result<()>,
) -> Result<()> {
    let _lock = lock_machine(margins_home)?;
    migrate_locked(margins_home)?;
    let path = machine_config_path(margins_home);
    let mut document = read_document(&path)?.unwrap_or_default();
    mutate(&mut document)?;
    atomic_write(&path, document.to_string().as_bytes())
}

/// Engine settings from `configs/settings.enzyme`, after migration; defaults
/// when the program does not exist.
pub fn engine_settings(margins_home: &Path) -> Result<enzyme_spec::Settings> {
    ensure_migrated(margins_home)?;
    Ok(read_settings_program(&settings_program_path(margins_home))?
        .map(|program| program.settings)
        .unwrap_or_default())
}

/// Select the catalyst generator (`settings { generation <mode> }`). Margins
/// also keeps engine auto-update off unless the program says otherwise.
pub fn set_generation(margins_home: &Path, mode: &str) -> Result<()> {
    if !GENERATION_MODES.contains(&mode) {
        bail!("unsupported catalyst mode '{mode}'");
    }
    let _lock = lock_machine(margins_home)?;
    migrate_locked(margins_home)?;
    let path = settings_program_path(margins_home);
    let mut program = read_settings_program(&path)?.unwrap_or_default();
    program.settings.generation = Some(mode.to_string());
    program.settings.updates.get_or_insert(false);
    atomic_write(&path, enzyme_spec::render_program(&program).as_bytes())
}

/// Make `configs/settings.enzyme` exist with `updates disabled` before Margins
/// runs `enzyme`: Margins ships the engine binary, so the engine must never
/// update itself. Other settings are kept; an explicit `updates enabled` is
/// turned off.
pub fn ensure_engine_settings(margins_home: &Path) -> Result<()> {
    let path = settings_program_path(margins_home);
    if !margins_home.join(LEGACY_MACHINE_CONFIG).exists()
        && read_settings_program(&path)?.is_some_and(|program| program.settings.updates == Some(false))
    {
        return Ok(());
    }
    let _lock = lock_machine(margins_home)?;
    migrate_locked(margins_home)?;
    let mut program = read_settings_program(&path)?.unwrap_or_default();
    if program.settings.updates == Some(false) && path.exists() {
        return Ok(());
    }
    if program.settings.updates == Some(true) {
        log::warn!(
            "{}: turning engine updates off; Margins updates the enzyme binary it ships",
            path.display()
        );
    }
    program.settings.updates = Some(false);
    atomic_write(&path, enzyme_spec::render_program(&program).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_settings_are_created_with_updates_disabled_and_kept() {
        let home = home();
        ensure_engine_settings(home.path()).unwrap();
        let path = settings_program_path(home.path());
        let settings = read_settings_program(&path).unwrap().unwrap().settings;
        assert_eq!(settings.updates, Some(false));
        assert_eq!(settings.generation, None);

        std::fs::write(&path, "settings {\n  generation hosted\n  updates enabled\n}\n").unwrap();
        ensure_engine_settings(home.path()).unwrap();
        let settings = read_settings_program(&path).unwrap().unwrap().settings;
        assert_eq!(settings.updates, Some(false));
        assert_eq!(settings.generation.as_deref(), Some("hosted"));

        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        ensure_engine_settings(home.path()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    fn home() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn full_legacy_config_splits_into_host_preferences_and_engine_settings() {
        let home = home();
        let legacy = "# my machine\n[workspace]\ndefault = \"practice\"\n\n[workspace.names]\npractice = \"Practice\"\n\n[retention]\ncaptures_days = 30\n\n[retention.practice]\ncaptures_days = 7\n\n[llm]\nmode = \"local\"\nlocal_model = \"fixture-model\"\n\n[audio]\ninput_name = \"Mic\"\n\n[cli]\nagent = \"codex\"\n";
        std::fs::write(home.path().join("config.toml"), legacy).unwrap();

        ensure_migrated(home.path()).unwrap();

        assert!(!home.path().join("config.toml").exists());
        assert_eq!(read(&home.path().join("config.toml.migrated")), legacy);
        let config = read(&home.path().join("margins.toml"));
        let parsed: toml::Table = toml::from_str(&config).unwrap();
        assert_eq!(parsed["workspace"]["default"].as_str(), Some("practice"));
        assert_eq!(parsed["workspace"]["names"]["practice"].as_str(), Some("Practice"));
        assert_eq!(parsed["retention"]["captures_days"].as_integer(), Some(30));
        assert_eq!(parsed["retention"]["practice"]["captures_days"].as_integer(), Some(7));
        assert_eq!(parsed["audio"]["input_name"].as_str(), Some("Mic"));
        assert_eq!(parsed["cli"]["agent"].as_str(), Some("codex"));
        assert!(parsed.get("llm").is_none());
        assert!(config.starts_with("# my machine"), "comments survive: {config}");
        let settings = engine_settings(home.path()).unwrap();
        assert_eq!(settings.generation.as_deref(), Some("local"));
        assert_eq!(settings.local_model.as_deref(), Some("fixture-model"));
        assert_eq!(settings.updates, Some(false));
        let settings_text = read(&home.path().join("configs/settings.enzyme"));
        assert!(settings_text.contains("generation local"), "{settings_text}");
        assert!(settings_text.contains("updates disabled"), "{settings_text}");
    }

    #[test]
    fn migration_is_idempotent_and_keeps_every_retired_original() {
        let home = home();
        std::fs::write(home.path().join("config.toml"), "[llm]\nmode = \"hosted\"\n").unwrap();
        ensure_migrated(home.path()).unwrap();
        let config = read(&home.path().join("margins.toml"));
        let settings = read(&home.path().join("configs/settings.enzyme"));
        ensure_migrated(home.path()).unwrap();
        assert_eq!(read(&home.path().join("margins.toml")), config);
        assert_eq!(read(&home.path().join("configs/settings.enzyme")), settings);

        // An earlier Margins writes a fresh legacy file after the migration:
        // its values win, and both originals are kept.
        std::fs::write(
            home.path().join("config.toml"),
            "[workspace]\ndefault = \"later\"\n[llm]\nmode = \"local\"\n",
        )
        .unwrap();
        ensure_migrated(home.path()).unwrap();
        assert!(home.path().join("config.toml.migrated").is_file());
        assert!(home.path().join("config.toml.migrated.1").is_file());
        let parsed: toml::Table = toml::from_str(&read(&home.path().join("margins.toml"))).unwrap();
        assert_eq!(parsed["workspace"]["default"].as_str(), Some("later"));
        assert_eq!(
            engine_settings(home.path()).unwrap().generation.as_deref(),
            Some("local")
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_never_loosens_margins_toml_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        let home = home();
        let legacy = home.path().join("config.toml");
        let config = home.path().join("margins.toml");
        std::fs::write(&legacy, "[cli]\nnote_agent = \"codex\"\n").unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o640)).unwrap();
        ensure_migrated(home.path()).unwrap();
        assert_eq!(mode(&config), 0o640, "a new margins.toml takes the legacy mode");

        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&legacy, "[cli]\nnote_agent = \"cursor\"\n").unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o644)).unwrap();
        ensure_migrated(home.path()).unwrap();
        assert_eq!(mode(&config), 0o600, "an existing margins.toml keeps its mode");
    }

    #[test]
    fn existing_margins_toml_and_settings_are_merged_not_replaced() {
        let home = home();
        std::fs::write(
            home.path().join("margins.toml"),
            "[workspace.names]\nkept = \"Kept\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(home.path().join("configs")).unwrap();
        std::fs::write(
            home.path().join("configs/settings.enzyme"),
            "settings {\n  updates enabled\n  embedding limit 50\n}\n",
        )
        .unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[workspace.names]\nother = \"Other\"\n",
        )
        .unwrap();

        ensure_migrated(home.path()).unwrap();

        let parsed: toml::Table = toml::from_str(&read(&home.path().join("margins.toml"))).unwrap();
        assert_eq!(parsed["workspace"]["names"]["kept"].as_str(), Some("Kept"));
        assert_eq!(parsed["workspace"]["names"]["other"].as_str(), Some("Other"));
        let settings = engine_settings(home.path()).unwrap();
        assert_eq!(settings.updates, Some(true), "an explicit choice is kept");
        assert_eq!(settings.embedding_limit, Some(50));
        assert_eq!(settings.generation, None);
    }

    #[test]
    fn invalid_legacy_configs_are_refused_without_writing() {
        for legacy in [
            "this is = = not toml\n",
            "[llm]\nmode = \"cloud\"\n",
            "[llm]\nmode = 3\n",
            "llm = \"local\"\n",
        ] {
            let home = home();
            std::fs::write(home.path().join("config.toml"), legacy).unwrap();
            assert!(ensure_migrated(home.path()).is_err(), "{legacy}");
            assert_eq!(read(&home.path().join("config.toml")), legacy);
            assert!(!home.path().join("margins.toml").exists());
            assert!(!home.path().join("configs").exists());
            assert!(!home.path().join("config.toml.migrated").exists());
        }
    }

    #[test]
    fn settings_program_with_other_declarations_is_refused() {
        let home = home();
        std::fs::create_dir_all(home.path().join("configs")).unwrap();
        std::fs::write(
            home.path().join("configs/settings.enzyme"),
            "workspace \"settings\" {\n  source markdown \"notes\" { path \"/abs/notes\" }\n}\n",
        )
        .unwrap();
        std::fs::write(home.path().join("config.toml"), "[llm]\nmode = \"local\"\n").unwrap();
        let error = ensure_migrated(home.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("margins workspace rename settings <new-id>"),
            "{error:#}"
        );
        assert!(home.path().join("config.toml").is_file());
        assert!(!home.path().join("margins.toml").exists());
    }

    #[test]
    fn empty_and_absent_legacy_configs() {
        let home = home();
        ensure_migrated(home.path()).unwrap();
        assert!(!home.path().join("margins.toml").exists());
        assert!(!home.path().join("configs").exists());

        std::fs::write(home.path().join("config.toml"), "  \n").unwrap();
        ensure_migrated(home.path()).unwrap();
        assert!(home.path().join("config.toml.migrated").is_file());
        assert_eq!(engine_settings(home.path()).unwrap().updates, Some(false));
    }

    #[test]
    fn set_generation_writes_the_settings_program() {
        let home = home();
        set_generation(home.path(), "hosted").unwrap();
        let settings = engine_settings(home.path()).unwrap();
        assert_eq!(settings.generation.as_deref(), Some("hosted"));
        assert_eq!(settings.updates, Some(false));
        assert!(!home.path().join("margins.toml").exists());
        assert!(set_generation(home.path(), "cloud").is_err());
    }

    #[test]
    fn concurrent_migrations_migrate_once() {
        let home = home();
        std::fs::write(
            home.path().join("config.toml"),
            "[workspace]\ndefault = \"practice\"\n[llm]\nmode = \"local\"\n",
        )
        .unwrap();
        let threads = (0..8)
            .map(|_| {
                let path = home.path().to_path_buf();
                std::thread::spawn(move || ensure_migrated(&path))
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        assert!(home.path().join("config.toml.migrated").is_file());
        assert!(!home.path().join("config.toml.migrated.1").exists());
        assert!(!home.path().join("config.toml").exists());
    }
}
