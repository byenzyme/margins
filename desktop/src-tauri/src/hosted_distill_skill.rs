use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

pub(crate) const DISTILL_SKILL_PATH_ENV: &str = "MARGINS_DISTILL_SKILL_PATH";

const MATERIALIZED_ROOT_DIR: &str = "hosted-distill-skills";
const HOST_PREAMBLE_PATH: &str = "hosts/desktop.md";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostedDistillCaller {
    ProcessSession,
    RefineSession,
}

impl HostedDistillCaller {
    fn command(self) -> &'static str {
        match self {
            Self::ProcessSession => "process_session",
            Self::RefineSession => "refine_session",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EmbeddedSkillFile {
    pub(crate) relative_path: &'static str,
    pub(crate) contents: &'static str,
}

const EMBEDDED_BUNDLE_FILES: &[EmbeddedSkillFile] = &[
    EmbeddedSkillFile {
        relative_path: "SKILL.md",
        contents: include_str!("../../../skills/margins/SKILL.md"),
    },
    EmbeddedSkillFile {
        relative_path: "distillation-core.md",
        contents: include_str!("../../../skills/margins/distillation-core.md"),
    },
    EmbeddedSkillFile {
        relative_path: HOST_PREAMBLE_PATH,
        contents: include_str!("../../../skills/margins/hosts/desktop.md"),
    },
    EmbeddedSkillFile {
        relative_path: "templates/1on1-idea-exchange.md",
        contents: include_str!("../../../skills/margins/templates/1on1-idea-exchange.md"),
    },
    EmbeddedSkillFile {
        relative_path: "templates/design-scoping-session.md",
        contents: include_str!("../../../skills/margins/templates/design-scoping-session.md"),
    },
    EmbeddedSkillFile {
        relative_path: "templates/discovery-call.md",
        contents: include_str!("../../../skills/margins/templates/discovery-call.md"),
    },
    EmbeddedSkillFile {
        relative_path: "templates/group-conversation.md",
        contents: include_str!("../../../skills/margins/templates/group-conversation.md"),
    },
    EmbeddedSkillFile {
        relative_path: "templates/talk-reflection.md",
        contents: include_str!("../../../skills/margins/templates/talk-reflection.md"),
    },
];

pub(crate) fn embedded_bundle_files() -> &'static [EmbeddedSkillFile] {
    EMBEDDED_BUNDLE_FILES
}

pub(crate) fn materialize_default_bundle(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let bundle_root = data_dir
        .join(MATERIALIZED_ROOT_DIR)
        .join(format!("margins-distill-{}", embedded_bundle_revision()));
    std::fs::create_dir_all(&bundle_root).with_context(|| {
        format!(
            "failed to create hosted distillation bundle directory {}",
            bundle_root.display()
        )
    })?;

    for file in EMBEDDED_BUNDLE_FILES {
        write_embedded_file_atomic(&bundle_root, file)?;
    }
    prune_unexpected_paths(&bundle_root)?;
    verify_materialized_bundle(&bundle_root)?;

    Ok(bundle_root.join(HOST_PREAMBLE_PATH))
}

pub(crate) fn resolve_for_state(
    state: &crate::AppState,
    caller: HostedDistillCaller,
) -> Result<PathBuf, String> {
    let default = state
        .hosted_distill_skill_path
        .lock()
        .map_err(|_| "hosted distillation skill path lock was poisoned".to_string())?
        .clone();
    resolve_hosted_distill_skill_path(default.as_deref(), caller)
}

pub(crate) fn resolve_hosted_distill_skill_path(
    materialized_default: Option<&Path>,
    caller: HostedDistillCaller,
) -> Result<PathBuf, String> {
    if let Some(explicit) = std::env::var_os(DISTILL_SKILL_PATH_ENV) {
        let path = PathBuf::from(explicit);
        validate_skill_bundle_host_path(&path).map_err(|error| {
            format!(
                "{DISTILL_SKILL_PATH_ENV} is set but invalid for {}: {error}",
                caller.command()
            )
        })?;
        return Ok(path);
    }

    let path = materialized_default.ok_or_else(|| {
        format!(
            "{} in server mode could not find the embedded Margins distillation bundle. Restart margins-server; startup should materialize it under MARGINS_DATA_DIR.",
            caller.command()
        )
    })?;
    validate_skill_bundle_host_path(path).map_err(|error| {
        format!(
            "embedded Margins distillation bundle is invalid for {}: {error}",
            caller.command()
        )
    })?;
    Ok(path.to_path_buf())
}

pub(crate) fn validate_skill_bundle_host_path(skill_path: &Path) -> Result<(), String> {
    if skill_path.as_os_str().is_empty() {
        return Err(format!(
            "expected a path to the Margins desktop host preamble, for example skills/margins/{HOST_PREAMBLE_PATH}"
        ));
    }
    if !skill_path.is_file() {
        return Err(format!(
            "host preamble is not a readable file: {}",
            skill_path.display()
        ));
    }
    let root = bundle_root_for_host_path(skill_path)?;
    let core_path = root.join("distillation-core.md");
    if !core_path.is_file() {
        return Err(format!(
            "shared distillation core is missing: {}",
            core_path.display()
        ));
    }
    let template_dir = root.join("templates");
    if !template_dir.is_dir() {
        return Err(format!(
            "template directory is missing: {}",
            template_dir.display()
        ));
    }
    for template in required_template_paths() {
        let path = root.join(template);
        if !path.is_file() {
            return Err(format!("required template is missing: {}", path.display()));
        }
    }
    Ok(())
}

fn bundle_root_for_host_path(skill_path: &Path) -> Result<PathBuf, String> {
    let skill_dir = skill_path
        .parent()
        .ok_or_else(|| "host preamble path has no parent directory".to_string())?;
    if skill_dir.join("templates").is_dir() && skill_dir.join("distillation-core.md").is_file() {
        return Ok(skill_dir.to_path_buf());
    }
    if skill_dir.file_name().and_then(|name| name.to_str()) == Some("hosts") {
        return skill_dir
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "host preamble path has no bundle root parent".to_string());
    }
    Err(format!(
        "could not locate bundle root next to {}; expected distillation-core.md and templates/ beside the host preamble, or a hosts/desktop.md path inside the bundle",
        skill_path.display()
    ))
}

fn required_template_paths() -> impl Iterator<Item = &'static str> {
    EMBEDDED_BUNDLE_FILES
        .iter()
        .map(|file| file.relative_path)
        .filter(|path| path.starts_with("templates/"))
}

fn embedded_bundle_revision() -> String {
    let mut hasher = Sha256::new();
    for file in EMBEDDED_BUNDLE_FILES {
        hasher.update(file.relative_path.as_bytes());
        hasher.update([0]);
        hasher.update(file.contents.as_bytes());
        hasher.update([0]);
    }
    format!("{:x}", hasher.finalize())
}

fn write_embedded_file_atomic(root: &Path, file: &EmbeddedSkillFile) -> anyhow::Result<()> {
    validate_bounded_relative_path(file.relative_path)?;
    let path = root.join(file.relative_path);
    let parent = path.parent().with_context(|| {
        format!(
            "embedded hosted distillation path has no parent: {}",
            file.relative_path
        )
    })?;
    std::fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create hosted distillation bundle directory {}",
            parent.display()
        )
    })?;

    let temp_path = create_temp_path(parent, &path)?;
    let result = (|| -> anyhow::Result<()> {
        let mut temp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| {
                format!(
                    "failed to create temporary hosted distillation file {}",
                    temp_path.display()
                )
            })?;
        temp.write_all(file.contents.as_bytes()).with_context(|| {
            format!(
                "failed to write temporary hosted distillation file {}",
                temp_path.display()
            )
        })?;
        temp.sync_all().with_context(|| {
            format!(
                "failed to sync temporary hosted distillation file {}",
                temp_path.display()
            )
        })?;
        drop(temp);
        std::fs::rename(&temp_path, &path).with_context(|| {
            format!(
                "failed to atomically publish hosted distillation file {}",
                path.display()
            )
        })?;
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn create_temp_path(parent: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bundle-file");
    let safe_name: String = file_name
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
        .take(64)
        .collect();
    let safe_name = if safe_name.is_empty() {
        "bundle-file"
    } else {
        &safe_name
    };
    for _ in 0..16 {
        let path = parent.join(format!(
            ".{safe_name}.tmp-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        if !path.exists() {
            return Ok(path);
        }
    }
    bail!(
        "failed to allocate a bounded temporary filename in {}",
        parent.display()
    )
}

fn validate_bounded_relative_path(path: &str) -> anyhow::Result<()> {
    if path.len() > 160 {
        bail!("embedded distillation path is too long: {path}");
    }
    let relative = Path::new(path);
    if relative.is_absolute() {
        bail!("embedded distillation path must be relative: {path}");
    }
    for component in relative.components() {
        match component {
            Component::Normal(name) if !name.is_empty() && name.to_string_lossy().len() <= 96 => {}
            _ => bail!("embedded distillation path contains an unsafe component: {path}"),
        }
    }
    Ok(())
}

fn prune_unexpected_paths(root: &Path) -> anyhow::Result<()> {
    let expected_files: HashSet<PathBuf> = EMBEDDED_BUNDLE_FILES
        .iter()
        .map(|file| PathBuf::from(file.relative_path))
        .collect();
    let mut expected_dirs = HashSet::from([PathBuf::new()]);
    for file in &expected_files {
        let mut current = PathBuf::new();
        for component in file.parent().into_iter().flat_map(|path| path.components()) {
            current.push(component.as_os_str());
            expected_dirs.insert(current.clone());
        }
    }
    prune_dir(root, root, &expected_files, &expected_dirs)
}

fn prune_dir(
    root: &Path,
    dir: &Path,
    expected_files: &HashSet<PathBuf>,
    expected_dirs: &HashSet<PathBuf>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| {
        format!(
            "failed to read hosted distillation directory {}",
            dir.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read hosted distillation directory entry in {}",
                dir.display()
            )
        })?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("failed to relativize {}", path.display()))?
            .to_path_buf();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if file_type.is_dir() {
            prune_dir(root, &path, expected_files, expected_dirs)?;
            if !expected_dirs.contains(&relative) {
                std::fs::remove_dir(&path).with_context(|| {
                    format!("failed to remove stale directory {}", path.display())
                })?;
            }
        } else if !expected_files.contains(&relative) {
            std::fs::remove_file(&path)
                .with_context(|| format!("failed to remove stale file {}", path.display()))?;
        }
    }
    Ok(())
}

fn verify_materialized_bundle(root: &Path) -> anyhow::Result<()> {
    for file in EMBEDDED_BUNDLE_FILES {
        let path = root.join(file.relative_path);
        let actual = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to verify hosted distillation file {}",
                path.display()
            )
        })?;
        if actual != file.contents {
            bail!(
                "hosted distillation file verification failed after materialization: {}",
                path.display()
            );
        }
    }
    validate_skill_bundle_host_path(&root.join(HOST_PREAMBLE_PATH))
        .map_err(anyhow::Error::msg)
        .with_context(|| {
            format!(
                "materialized hosted distillation bundle is incomplete at {}",
                root.display()
            )
        })
}

fn sync_directory(path: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|directory| directory.sync_all())
            .with_context(|| format!("failed to sync directory {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct EnvGuard {
        prior: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn unset() -> Self {
            let prior = std::env::var_os(DISTILL_SKILL_PATH_ENV);
            std::env::remove_var(DISTILL_SKILL_PATH_ENV);
            Self { prior }
        }

        fn set(path: &Path) -> Self {
            let prior = std::env::var_os(DISTILL_SKILL_PATH_ENV);
            std::env::set_var(DISTILL_SKILL_PATH_ENV, path);
            Self { prior }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(prior) = &self.prior {
                std::env::set_var(DISTILL_SKILL_PATH_ENV, prior);
            } else {
                std::env::remove_var(DISTILL_SKILL_PATH_ENV);
            }
        }
    }

    #[test]
    fn embedded_bundle_contains_every_required_production_file() {
        let paths: BTreeSet<_> = embedded_bundle_files()
            .iter()
            .map(|file| file.relative_path)
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from([
                "SKILL.md",
                "distillation-core.md",
                "hosts/desktop.md",
                "templates/1on1-idea-exchange.md",
                "templates/design-scoping-session.md",
                "templates/discovery-call.md",
                "templates/group-conversation.md",
                "templates/talk-reflection.md",
            ])
        );
        for file in embedded_bundle_files() {
            assert!(!file.contents.trim().is_empty(), "{}", file.relative_path);
        }
    }

    #[test]
    fn materialization_creates_exact_tree_and_replaces_stale_contents() {
        let dir = tempfile::tempdir().unwrap();
        let host = materialize_default_bundle(dir.path()).unwrap();
        let root = host.parent().unwrap().parent().unwrap().to_path_buf();
        assert_exact_materialized_tree(&root);

        std::fs::write(root.join("distillation-core.md"), "stale core").unwrap();
        std::fs::write(root.join("templates/stale.md"), "stale template").unwrap();
        let stale_dir = root.join("old");
        std::fs::create_dir_all(&stale_dir).unwrap();
        std::fs::write(stale_dir.join("file.md"), "stale").unwrap();

        let host_again = materialize_default_bundle(dir.path()).unwrap();
        assert_eq!(host_again, host);
        assert_eq!(
            std::fs::read_to_string(root.join("distillation-core.md")).unwrap(),
            embedded_bundle_files()
                .iter()
                .find(|file| file.relative_path == "distillation-core.md")
                .unwrap()
                .contents
        );
        assert!(!root.join("templates/stale.md").exists());
        assert!(!stale_dir.exists());
        assert_exact_materialized_tree(&root);
    }

    #[test]
    fn valid_explicit_override_wins() {
        let _guard = env_lock().lock().unwrap();
        let default_dir = tempfile::tempdir().unwrap();
        let explicit_dir = tempfile::tempdir().unwrap();
        let default_host = materialize_default_bundle(default_dir.path()).unwrap();
        let explicit_host = materialize_default_bundle(explicit_dir.path()).unwrap();
        let _env = EnvGuard::set(&explicit_host);

        assert_eq!(
            resolve_hosted_distill_skill_path(
                Some(&default_host),
                HostedDistillCaller::ProcessSession
            )
            .unwrap(),
            explicit_host
        );
    }

    #[test]
    fn invalid_explicit_override_errors_without_falling_back() {
        let _guard = env_lock().lock().unwrap();
        let default_dir = tempfile::tempdir().unwrap();
        let default_host = materialize_default_bundle(default_dir.path()).unwrap();
        let invalid_dir = tempfile::tempdir().unwrap();
        let invalid_host = invalid_dir.path().join("hosts/desktop.md");
        std::fs::create_dir_all(invalid_host.parent().unwrap()).unwrap();
        std::fs::write(&invalid_host, "# host only").unwrap();
        let _env = EnvGuard::set(&invalid_host);

        let error = resolve_hosted_distill_skill_path(
            Some(&default_host),
            HostedDistillCaller::ProcessSession,
        )
        .unwrap_err();
        assert!(error.contains(DISTILL_SKILL_PATH_ENV), "{error}");
        assert!(error.contains("distillation-core.md"), "{error}");
        assert_ne!(PathBuf::from(error), default_host);
    }

    #[test]
    fn unset_server_path_uses_embedded_materialized_bundle() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::unset();
        let dir = tempfile::tempdir().unwrap();
        let host = materialize_default_bundle(dir.path()).unwrap();

        assert_eq!(
            resolve_hosted_distill_skill_path(Some(&host), HostedDistillCaller::ProcessSession)
                .unwrap(),
            host
        );
    }

    #[test]
    fn process_and_refine_share_the_same_resolver_policy() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::unset();
        let dir = tempfile::tempdir().unwrap();
        let host = materialize_default_bundle(dir.path()).unwrap();

        let process =
            resolve_hosted_distill_skill_path(Some(&host), HostedDistillCaller::ProcessSession)
                .unwrap();
        let refine =
            resolve_hosted_distill_skill_path(Some(&host), HostedDistillCaller::RefineSession)
                .unwrap();
        assert_eq!(process, refine);

        std::env::set_var(DISTILL_SKILL_PATH_ENV, dir.path().join("missing.md"));
        let process_error =
            resolve_hosted_distill_skill_path(Some(&host), HostedDistillCaller::ProcessSession)
                .unwrap_err();
        let refine_error =
            resolve_hosted_distill_skill_path(Some(&host), HostedDistillCaller::RefineSession)
                .unwrap_err();
        assert!(process_error.contains(DISTILL_SKILL_PATH_ENV));
        assert!(refine_error.contains(DISTILL_SKILL_PATH_ENV));
        assert!(process_error.contains("process_session"));
        assert!(refine_error.contains("refine_session"));
    }

    fn assert_exact_materialized_tree(root: &Path) {
        let mut actual = BTreeSet::new();
        collect_files(root, root, &mut actual);
        let expected: BTreeSet<_> = embedded_bundle_files()
            .iter()
            .map(|file| PathBuf::from(file.relative_path))
            .collect();
        assert_eq!(actual, expected);
        validate_skill_bundle_host_path(&root.join(HOST_PREAMBLE_PATH)).unwrap();
    }

    fn collect_files(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_files(root, &path, out);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
}
