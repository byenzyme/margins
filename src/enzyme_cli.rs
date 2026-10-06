//! The Enzyme engine, run as the shipped `enzyme` CLI.
//!
//! Every call runs with `ENZYME_HOME=$MARGINS_HOME`, so `enzyme` reads the
//! Workspace programs, `settings.enzyme`, and `margins-sources.enzyme` from
//! `$MARGINS_HOME/configs/` and keeps each index at
//! `$MARGINS_HOME/workspaces/<id>/enzyme.db`. The child environment is built
//! from an allowlist: no inherited `ENZYME_*`, `OPENAI_*`, or provider
//! variables reach it, and the generator is always explicit (`--llm env` with
//! Margins' hosted bundle as `OPENAI_*`, `--llm local`, or `--llm none`), so
//! `enzyme` never discovers stored Enzyme auth or shared caches.
//!
//! Output contracts are the engine's versioned JSON envelopes
//! (`docs/cli-json.md` in enzyme-rust) and its exit codes
//! (`docs/cli-exit-codes.md`), mapped here to [`EngineError`].

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};

/// Explicit `enzyme` binary; checked first.
pub const ENZYME_BIN_ENV: &str = "MARGINS_ENZYME_BIN";
/// Seconds a foreground `init`/`refresh` waits for another holder of the
/// workspace lock (engine default 1800). `0` fails at once.
pub const LOCK_TIMEOUT_ENV: &str = "MARGINS_ENZYME_LOCK_TIMEOUT";

pub const SEARCH_SCHEMA: &str = "enzyme.search.v1";
pub const STATUS_SCHEMA: &str = "enzyme.status.v1";
pub const PROFILES_SCHEMA: &str = "enzyme.profiles.v1";
pub const MODELS_SCHEMA: &str = "enzyme.models.v1";
pub const COMPILE_SCHEMA: &str = "compile.v2";

/// Parent variables the child may inherit. Everything else, notably
/// `ENZYME_*`, `OPENAI_*`, and `OPENROUTER_*`, is dropped.
const INHERITED_ENV: &[&str] = &[
    "HOME",
    "PATH",
    "TMPDIR",
    "TMP",
    "TEMP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "USER",
    "LOGNAME",
    "SYSTEMROOT",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    // Network reach for `--llm env`, as the in-process engine had it. These
    // carry no credentials Margins did not already put there.
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "all_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// The `enzyme --version` this Margins requires, from `scripts/enzyme-cli.pin`.
pub fn required_version() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        include_str!("../scripts/enzyme-cli.pin")
            .parse::<toml::Table>()
            .expect("scripts/enzyme-cli.pin is TOML")
            .get("version")
            .and_then(toml::Value::as_str)
            .expect("scripts/enzyme-cli.pin names a version")
            .to_string()
    })
}

/// A failed engine call, by the engine's exit-code contract.
#[derive(Debug)]
pub enum EngineError {
    /// No candidate was a usable `enzyme` of the pinned version; each path
    /// with why it was rejected.
    NotFound { rejected: Vec<(PathBuf, Rejection)> },
    /// [`ENZYME_BIN_ENV`] names an `enzyme` that is not the pinned release.
    VersionMismatch {
        bin: PathBuf,
        found: String,
        expected: String,
    },
    /// Exit 2: bad arguments.
    Usage(String),
    /// Exit 3: a program or the workspace is not usable as configured: parse
    /// or resolve errors, a missing source folder or database, a generator
    /// that is not available, or an index that does not exist yet.
    Config(String),
    /// Exit 4: another `init`/`refresh` of the workspace held its lock past
    /// the timeout. Nothing was changed.
    Busy(String),
    /// Exit 5: `init` built the index (search works) but catalyst generation
    /// failed its quality gate.
    CatalystsFailed(String),
    /// Exit 1 or any other failure.
    Failed { code: Option<i32>, message: String },
    /// The output did not match the documented envelope.
    Protocol(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { rejected } => write!(
                f,
                "no usable enzyme {} engine: {}; reinstall Margins or set {ENZYME_BIN_ENV}",
                required_version(),
                rejected
                    .iter()
                    .map(|(path, why)| format!("{} ({why})", path.display()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::VersionMismatch {
                bin,
                found,
                expected,
            } => write!(
                f,
                "{} is enzyme {found}, but this Margins runs enzyme {expected}; reinstall Margins (or point {ENZYME_BIN_ENV} at enzyme {expected})",
                bin.display()
            ),
            Self::Usage(message) => write!(f, "enzyme rejected its arguments: {message}"),
            Self::Config(message) => write!(f, "{message}"),
            Self::Busy(message) => write!(
                f,
                "workspace busy: another index build is still running ({message}); try again when it finishes"
            ),
            Self::CatalystsFailed(message) => write!(
                f,
                "the index was built, but catalyst generation failed: {message}"
            ),
            Self::Failed { code, message } => match code {
                Some(code) => write!(f, "enzyme failed (exit {code}): {message}"),
                None => write!(f, "enzyme was terminated: {message}"),
            },
            Self::Protocol(message) => write!(f, "unexpected enzyme output: {message}"),
        }
    }
}

impl std::error::Error for EngineError {}

/// Which catalyst generator a call may use.
#[derive(Clone, PartialEq, Eq)]
pub enum Generator {
    /// An OpenAI-compatible endpoint passed only to the child.
    Env {
        api_key: String,
        base_url: String,
        model: String,
    },
    /// The selected local model in `$MARGINS_HOME/models`.
    Local,
    /// Index, embed, and select only.
    None,
}

impl std::fmt::Debug for Generator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.mode())
    }
}

impl Generator {
    pub fn mode(&self) -> &'static str {
        match self {
            Self::Env { .. } => "env",
            Self::Local => "local",
            Self::None => "none",
        }
    }

    pub fn generates(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Why a candidate `enzyme` was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    Missing,
    NotExecutable,
    /// It ran, but reported this version (or `unknown`).
    Version(String),
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("missing"),
            Self::NotExecutable => f.write_str("not executable"),
            Self::Version(found) => write!(f, "version {found}"),
        }
    }
}

/// Locate the pinned `enzyme`, once per process and Margins home.
///
/// [`ENZYME_BIN_ENV`], when set, is used or the call fails. Otherwise the
/// bundled locations are tried in order: the installers'
/// `<exe dir>/../libexec/margins/enzyme` (Homebrew keg, `~/.local`
/// installs) when `<exe dir>/../libexec/margins` exists, otherwise
/// `<exe dir>/enzyme` (the release archive); then `$MARGINS_HOME/bin/enzyme`.
/// A candidate that is missing, not executable, or not the pinned version is
/// skipped. An installed `margins` therefore never runs an `enzyme` beside it
/// in a shared `bin` directory such as `~/.local/bin`, which may be the user's
/// own release that updates itself, and `PATH` is never searched.
pub fn locate_binary(margins_home: &Path) -> Result<PathBuf, EngineError> {
    static FOUND: OnceLock<Mutex<BTreeMap<(Option<PathBuf>, PathBuf), PathBuf>>> = OnceLock::new();
    let explicit = std::env::var_os(ENZYME_BIN_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let key = (explicit.clone(), margins_home.to_path_buf());
    let mut found = FOUND
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(bin) = found.get(&key) {
        return Ok(bin.clone());
    }
    let bin = match explicit {
        Some(explicit) => select_explicit(&explicit)?,
        None => select_bundled(&bundled_candidates(margins_home))?,
    };
    found.insert(key, bin.clone());
    Ok(bin)
}

fn bundled_candidates(margins_home: &Path) -> Vec<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.canonicalize().ok())
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    candidates_for(exe_dir.as_deref(), margins_home)
}

/// The bundled locations for a `margins` in `exe_dir`: the installed layout's
/// libexec copy, or, when there is no `../libexec/margins`, the archive
/// layout's sibling.
fn candidates_for(exe_dir: Option<&Path>, margins_home: &Path) -> Vec<PathBuf> {
    let name = format!("enzyme{}", std::env::consts::EXE_SUFFIX);
    let mut candidates = Vec::new();
    if let Some(dir) = exe_dir {
        let libexec = dir
            .parent()
            .map(|prefix| prefix.join("libexec").join("margins"))
            .filter(|libexec| libexec.is_dir());
        candidates.push(match libexec {
            Some(libexec) => libexec.join(&name),
            None => dir.join(&name),
        });
    }
    candidates.push(margins_home.join("bin").join(&name));
    candidates
}

/// The explicit binary is the only candidate; a wrong version is an error.
fn select_explicit(bin: &Path) -> Result<PathBuf, EngineError> {
    match probe(bin) {
        Ok(()) => Ok(bin.to_path_buf()),
        Err(Rejection::Version(found)) => Err(EngineError::VersionMismatch {
            bin: bin.to_path_buf(),
            found,
            expected: required_version().to_string(),
        }),
        Err(why) => Err(EngineError::NotFound {
            rejected: vec![(bin.to_path_buf(), why)],
        }),
    }
}

/// The first candidate that is the pinned version.
fn select_bundled(candidates: &[PathBuf]) -> Result<PathBuf, EngineError> {
    let mut rejected = Vec::new();
    for candidate in candidates {
        match probe(candidate) {
            Ok(()) => return Ok(candidate.clone()),
            Err(why) => rejected.push((candidate.clone(), why)),
        }
    }
    Err(EngineError::NotFound { rejected })
}

fn probe(path: &Path) -> Result<(), Rejection> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return Err(Rejection::Missing);
    };
    if !is_executable(path) {
        return Err(if metadata.is_file() {
            Rejection::NotExecutable
        } else {
            Rejection::Missing
        });
    }
    match reported_version(path) {
        Some(found) if found == required_version() => Ok(()),
        Some(found) => Err(Rejection::Version(found)),
        None => Err(Rejection::Version("unknown".to_string())),
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

/// What `enzyme --version` reports, run once per binary per process; `None`
/// when it does not run or exits nonzero.
fn reported_version(bin: &Path) -> Option<String> {
    static REPORTED: OnceLock<Mutex<BTreeMap<PathBuf, Option<String>>>> = OnceLock::new();
    let mut reported = REPORTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reported
        .entry(bin.to_path_buf())
        .or_insert_with(|| {
            let output = Command::new(bin)
                .arg("--version")
                .env_clear()
                .stdin(Stdio::null())
                .output()
                .ok()
                .filter(|output| output.status.success())?;
            let text = String::from_utf8_lossy(&output.stdout);
            let text = text.trim();
            let found = text.strip_prefix("enzyme ").unwrap_or(text);
            (!found.is_empty()).then(|| found.to_string())
        })
        .clone()
}

/// One Margins home driving `enzyme`.
#[derive(Debug, Clone)]
pub struct Engine {
    bin: PathBuf,
    home: PathBuf,
}

impl Engine {
    /// Locate the binary and make the home ready for the engine: legacy
    /// machine config migrated, `settings.enzyme` with updates disabled, and
    /// the managed `margins-sources.enzyme`.
    pub fn for_home(margins_home: &Path) -> Result<Self> {
        let bin = locate_binary(margins_home)?;
        margins_workflows::machine_config::ensure_engine_settings(margins_home)?;
        margins_workflows::source_kinds::ensure_sources_program(margins_home)?;
        Ok(Self {
            bin,
            home: margins_home.to_path_buf(),
        })
    }

    /// Locate the binary for read-only engine queries (such as
    /// [`Engine::models`]) without preparing the home: nothing is migrated or
    /// written, so status and diagnostic commands leave the home untouched.
    pub fn for_inspection(margins_home: &Path) -> Result<Self> {
        Ok(Self {
            bin: locate_binary(margins_home)?,
            home: margins_home.to_path_buf(),
        })
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    fn command(&self, workspace: Option<&str>) -> Command {
        let mut command = Command::new(&self.bin);
        command.env_clear();
        for name in INHERITED_ENV {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("ENZYME_HOME", &self.home)
            .env("NO_COLOR", "1")
            .stdin(Stdio::null());
        if let Some(workspace) = workspace {
            command.arg("--workspace").arg(workspace);
        }
        command
    }

    fn generator_args(command: &mut Command, generator: &Generator) {
        command.arg("--llm").arg(generator.mode());
        if let Generator::Env {
            api_key,
            base_url,
            model,
        } = generator
        {
            command
                .env("OPENAI_API_KEY", api_key)
                .env("OPENAI_BASE_URL", base_url)
                .env("OPENAI_MODEL", model);
        }
    }

    /// `--lock-timeout`: [`LOCK_TIMEOUT_ENV`] when set, otherwise `default`
    /// seconds, otherwise the engine's own default.
    fn lock_args(command: &mut Command, default: Option<u64>) {
        if let Some(timeout) = std::env::var_os(LOCK_TIMEOUT_ENV).filter(|value| !value.is_empty())
        {
            command.arg("--lock-timeout").arg(timeout);
        } else if let Some(seconds) = default {
            command.arg("--lock-timeout").arg(seconds.to_string());
        }
    }

    fn run(&self, mut command: Command) -> Result<Output, EngineError> {
        // Arguments only: the environment carries credentials.
        debug(&format!(
            "{} {}",
            self.bin.display(),
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        command.output().map_err(|error| EngineError::Failed {
            code: None,
            message: format!("starting {}: {error}", self.bin.display()),
        })
    }

    /// Build or update the workspace index and generate catalysts in the
    /// foreground (`init --json-progress`).
    pub fn init(
        &self,
        workspace: &str,
        generator: &Generator,
        force: bool,
        lock_timeout: Option<u64>,
    ) -> Result<InitSummary, EngineError> {
        let mut command = self.command(Some(workspace));
        command.arg("init").arg("--json-progress");
        Self::generator_args(&mut command, generator);
        Self::lock_args(&mut command, lock_timeout);
        if force {
            command.arg("--force");
        }
        let output = self.run(command)?;
        let events = progress_events(&output.stderr);
        let summary = parse_json::<serde_json::Value>(&output.stdout).ok();
        if let Err(error) = check_status(&output, &events) {
            return Err(error);
        }
        let summary = summary.ok_or_else(|| {
            EngineError::Protocol(format!(
                "init printed no summary: {}",
                String::from_utf8_lossy(&output.stdout)
            ))
        })?;
        Ok(InitSummary::new(summary, &events))
    }

    /// Bring the index up to date (`refresh --quiet`). A due catalyst epoch is
    /// built by a detached worker that inherits this call's environment.
    pub fn refresh(
        &self,
        workspace: &str,
        generator: &Generator,
        lock_timeout: Option<u64>,
    ) -> Result<serde_json::Value, EngineError> {
        let mut command = self.command(Some(workspace));
        command.arg("refresh").arg("--quiet");
        Self::generator_args(&mut command, generator);
        Self::lock_args(&mut command, lock_timeout);
        let output = self.run(command)?;
        check_status(&output, &[])?;
        parse_json(&output.stdout)
    }

    /// `search --json`: catalyst, direct, and exact-phrase hits.
    pub fn search(
        &self,
        workspace: &str,
        query: &str,
        phrase: Option<&str>,
        limit: usize,
    ) -> Result<SearchEnvelope, EngineError> {
        let mut command = self.command(Some(workspace));
        command
            .arg("search")
            .arg(query)
            .arg("-n")
            .arg(limit.max(1).to_string())
            .arg("--json");
        if let Some(phrase) = phrase {
            command.arg("--phrase").arg(phrase);
        }
        let output = self.run(command)?;
        check_status(&output, &[])?;
        let envelope: SearchEnvelope = parse_json(&output.stdout)?;
        expect_schema(&envelope.schema, SEARCH_SCHEMA)?;
        Ok(envelope)
    }

    /// `status --json`; never creates or migrates the index.
    pub fn status(&self, workspace: &str) -> Result<StatusEnvelope, EngineError> {
        let mut command = self.command(Some(workspace));
        command.arg("status").arg("--json");
        let output = self.run(command)?;
        check_status(&output, &[])?;
        let envelope: StatusEnvelope = parse_json(&output.stdout)?;
        expect_schema(&envelope.schema, STATUS_SCHEMA)?;
        Ok(envelope)
    }

    /// `spec profiles --json`: the built-in catalyst profiles and formats.
    pub fn profiles(&self) -> Result<serde_json::Value, EngineError> {
        let mut command = self.command(None);
        command.args(["spec", "profiles", "--json"]);
        let output = self.run(command)?;
        check_status(&output, &[])?;
        let envelope: serde_json::Value = parse_json(&output.stdout)?;
        expect_schema(
            envelope
                .get("schema")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            PROFILES_SCHEMA,
        )?;
        Ok(envelope)
    }

    /// `compile --preset <template> --dry-run --json <source>`: the template
    /// filled for `workspace`, checked against the other programs in
    /// `configs/`. Nothing is saved.
    pub fn compile_preset(
        &self,
        workspace: &str,
        template: &Path,
        source: &Path,
    ) -> Result<String, EngineError> {
        let mut command = self.command(Some(workspace));
        command
            .arg("compile")
            .arg("--preset")
            .arg(template)
            .args(["--dry-run", "--json"])
            .arg(source);
        let output = self.run(command)?;
        check_status(&output, &[])?;
        let envelope: serde_json::Value = parse_json(&output.stdout)?;
        expect_schema(
            envelope
                .get("schema_version")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            COMPILE_SCHEMA,
        )?;
        envelope
            .get("program")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Protocol("compile printed no program".into()))
    }

    /// `model list --json`: the registry and what is installed in
    /// `$MARGINS_HOME/models`.
    pub fn models(&self) -> Result<ModelsEnvelope, EngineError> {
        let mut command = self.command(None);
        command.args(["model", "list", "--json"]);
        let output = self.run(command)?;
        check_status(&output, &[])?;
        let envelope: ModelsEnvelope = parse_json(&output.stdout)?;
        expect_schema(&envelope.schema, MODELS_SCHEMA)?;
        Ok(envelope)
    }
}

/// The `init` summary plus what its progress events reported.
#[derive(Debug, Clone)]
pub struct InitSummary {
    pub status: String,
    pub warnings: Vec<String>,
    /// Entities whose catalysts this run (re)generated.
    pub entities_generated: usize,
    pub raw: serde_json::Value,
}

impl InitSummary {
    fn new(raw: serde_json::Value, events: &[serde_json::Value]) -> Self {
        Self {
            status: raw
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            warnings: raw
                .get("warnings")
                .and_then(serde_json::Value::as_array)
                .map(|warnings| {
                    warnings
                        .iter()
                        .map(|warning| match warning.as_str() {
                            Some(text) => text.to_string(),
                            None => warning.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            entities_generated: events
                .iter()
                .filter(|event| {
                    event.get("stage").and_then(serde_json::Value::as_str) == Some("catalysts")
                        && event.get("status").and_then(serde_json::Value::as_str)
                            == Some("entity_done")
                })
                .count(),
            raw,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchEnvelope {
    pub schema: String,
    /// `ready` when the index has catalysts, otherwise `no_generator`.
    pub bridged: String,
    pub document_count: usize,
    #[serde(default)]
    pub catalyst_hits: Vec<SearchHit>,
    #[serde(default)]
    pub direct_hits: Vec<SearchHit>,
    #[serde(default)]
    pub exact_hits: Vec<SearchHit>,
    #[serde(default)]
    pub top_catalysts: Vec<TopCatalyst>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f64,
    pub content: String,
    #[serde(default)]
    pub via_catalyst_id: Option<String>,
    #[serde(default)]
    pub via_catalyst_text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TopCatalyst {
    pub id: String,
    pub text: String,
    pub entity: String,
    #[serde(default)]
    pub topic_name: Option<String>,
    pub relevance_score: f64,
    pub contribution_count: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StatusEnvelope {
    pub schema: String,
    pub initialized: bool,
    #[serde(default)]
    pub schema_outdated: bool,
    #[serde(default)]
    pub documents: usize,
    #[serde(default)]
    pub catalysts: usize,
    #[serde(default)]
    pub sources: Vec<SourceStatus>,
    #[serde(default)]
    pub markdown: Option<MarkdownStatus>,
    #[serde(default)]
    pub selection: Option<Selection>,
    #[serde(default)]
    pub update: Option<UpdateStatus>,
}

impl StatusEnvelope {
    /// Whether `init` must rebuild the index from scratch before it can be
    /// read: an older schema, or an index the engine says must regenerate.
    pub fn needs_rebuild(&self) -> bool {
        self.initialized
            && (self.schema_outdated
                || self
                    .update
                    .as_ref()
                    .is_some_and(|update| update.regen_required))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceStatus {
    pub name: String,
    #[serde(default)]
    pub documents: usize,
    #[serde(default)]
    pub last_refresh_ms: Option<i64>,
    #[serde(default)]
    pub stale: bool,
    #[serde(default)]
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MarkdownStatus {
    #[serde(default)]
    pub files_new: usize,
    #[serde(default)]
    pub files_modified: usize,
    #[serde(default)]
    pub files_deleted: usize,
    #[serde(default)]
    pub stale: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Selection {
    #[serde(default)]
    pub selected: usize,
    #[serde(default)]
    pub ready: usize,
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub skipped: usize,
    #[serde(default)]
    pub entities: Vec<SelectedEntity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SelectedEntity {
    pub name: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub state: String,
    #[serde(default)]
    pub catalysts: usize,
    #[serde(default)]
    pub skip_reason: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStatus {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub regen_required: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsEnvelope {
    pub schema: String,
    #[serde(default)]
    pub selected: Option<String>,
    /// The model `--llm local` would use.
    #[serde(default)]
    pub active: Option<String>,
    pub models_dir: PathBuf,
    #[serde(default)]
    pub models: Vec<ModelEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub installed: bool,
    #[serde(default)]
    pub path: Option<PathBuf>,
    pub size_bytes: u64,
    #[serde(default)]
    pub registry: bool,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
}

fn progress_events(stderr: &[u8]) -> Vec<serde_json::Value> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Map a non-zero exit to [`EngineError`], preferring the JSON error event's
/// message, then the last `Error: …` line on stderr.
fn check_status(output: &Output, events: &[serde_json::Value]) -> Result<(), EngineError> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = events
        .iter()
        .rev()
        .find(|event| event.get("stage").and_then(serde_json::Value::as_str) == Some("error"))
        .and_then(|event| event.get("message").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .or_else(|| {
            stderr
                .lines()
                .rev()
                .find_map(|line| line.strip_prefix("Error: "))
                .map(str::to_string)
        })
        .or_else(|| {
            stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "no error message".to_string());
    debug(&format!(
        "exit={:?} stderr={}",
        output.status.code(),
        stderr.trim()
    ));
    Err(match output.status.code() {
        Some(2) => EngineError::Usage(message),
        Some(3) => EngineError::Config(message),
        Some(4) => EngineError::Busy(message),
        Some(5) => EngineError::CatalystsFailed(message),
        code => EngineError::Failed { code, message },
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(stdout: &[u8]) -> Result<T, EngineError> {
    serde_json::from_slice(stdout).map_err(|error| {
        EngineError::Protocol(format!(
            "{error}: {}",
            String::from_utf8_lossy(stdout)
                .chars()
                .take(400)
                .collect::<String>()
        ))
    })
}

fn expect_schema(actual: &str, expected: &str) -> Result<(), EngineError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EngineError::Protocol(format!(
            "expected schema {expected}, got {actual:?}; this Margins needs a matching enzyme release"
        )))
    }
}

/// The generator Margins' setup selected, as the engine runs it: the hosted
/// bundle through `--llm env`, an installed local model through `--llm
/// local`, and otherwise no generation.
pub fn selected_generator(engine: &Engine, margins_home: &Path) -> Result<Generator> {
    use margins_workflows::catalyst::CatalystMode;
    let status = margins_workflows::catalyst::selected_status(margins_home);
    Ok(match status.mode {
        CatalystMode::Hosted => {
            match crate::hosted_credentials::cached_bundle_for_generation(margins_home)? {
                Some(bundle) => Generator::Env {
                    api_key: bundle.api_key,
                    base_url: bundle.base_url,
                    model: bundle.model,
                },
                None => Generator::None,
            }
        }
        CatalystMode::Local => {
            if engine
                .models()
                .context("listing local catalyst models")?
                .active
                .is_some()
            {
                Generator::Local
            } else {
                Generator::None
            }
        }
        CatalystMode::None => Generator::None,
    })
}

fn debug(message: &str) {
    if std::env::var_os("MARGINS_RECALL_DEBUG").is_some() {
        eprintln!("recall: enzyme {message}");
    }
}

/// Inherited variable names, for tests.
#[cfg(test)]
pub(crate) fn inherited_env() -> &'static [&'static str] {
    INHERITED_ENV
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn exit_codes_map_to_typed_errors() {
        for (code, expected) in [
            (2, "Usage"),
            (3, "Config"),
            (4, "Busy"),
            (5, "CatalystsFailed"),
            (1, "Failed"),
            (9, "Failed"),
        ] {
            let error =
                check_status(&output(code, "", "noise\nError: the reason\n"), &[]).unwrap_err();
            assert!(
                format!("{error:?}").starts_with(expected),
                "{code}: {error:?}"
            );
            assert!(error.to_string().contains("the reason"), "{error}");
        }
        assert!(check_status(&output(0, "{}", ""), &[]).is_ok());
    }

    #[test]
    fn json_progress_error_event_wins() {
        let stderr = "{\"stage\":\"indexing\",\"status\":\"start\"}\n{\"stage\":\"error\",\"exit_code\":4,\"message\":\"held by pid 7\"}\nError: held by pid 7\n";
        let events = progress_events(stderr.as_bytes());
        let error = check_status(&output(4, "", stderr), &events).unwrap_err();
        assert!(matches!(&error, EngineError::Busy(message) if message == "held by pid 7"));
    }

    #[test]
    fn init_summary_counts_generated_entities() {
        let events = progress_events(
            b"{\"stage\":\"catalysts\",\"status\":\"entity_started\",\"entity\":\"a\"}\n{\"stage\":\"catalysts\",\"status\":\"entity_done\",\"entity\":\"a\"}\n{\"stage\":\"catalysts\",\"status\":\"entity_done\",\"entity\":\"b\"}\n",
        );
        let summary = InitSummary::new(
            serde_json::json!({"status": "completed_with_warnings", "warnings": ["thin"]}),
            &events,
        );
        assert_eq!(summary.entities_generated, 2);
        assert_eq!(summary.status, "completed_with_warnings");
        assert_eq!(summary.warnings, ["thin"]);
    }

    #[test]
    fn generator_debug_never_prints_credentials() {
        let generator = Generator::Env {
            api_key: "secret".into(),
            base_url: "http://x".into(),
            model: "m".into(),
        };
        assert_eq!(format!("{generator:?}"), "env");
    }

    #[test]
    fn child_environment_drops_engine_and_provider_variables() {
        for name in inherited_env() {
            assert!(!name.starts_with("ENZYME_") && !name.starts_with("OPENAI_"));
        }
    }

    #[test]
    fn pinned_version_is_semver() {
        assert!(
            required_version().split('.').count() == 3,
            "{}",
            required_version()
        );
    }

    fn fake_enzyme(path: &Path, version: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\necho 'enzyme {version}'\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn version_mismatch_and_non_executables_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("enzyme");
        fake_enzyme(&fake, "0.0.1", 0o644);
        assert_eq!(probe(&fake), Err(Rejection::NotExecutable));
        let stale = dir.path().join("enzyme-stale");
        fake_enzyme(&stale, "0.0.1", 0o755);
        assert_eq!(probe(&stale), Err(Rejection::Version("0.0.1".into())));
        let error = select_explicit(&stale).unwrap_err();
        assert!(
            matches!(&error, EngineError::VersionMismatch { found, .. } if found == "0.0.1"),
            "{error:?}"
        );
        assert!(error.to_string().contains(required_version()), "{error}");
        let matching = dir.path().join("enzyme-ok");
        fake_enzyme(&matching, required_version(), 0o755);
        assert_eq!(select_explicit(&matching).unwrap(), matching);
    }

    #[test]
    fn mismatched_sibling_falls_through_to_matching_libexec_copy() {
        // `~/.local/bin/enzyme` is the user's own release; the installed
        // engine is `~/.local/libexec/margins/enzyme`.
        let prefix = tempfile::tempdir().unwrap();
        let libexec = prefix.path().join("libexec/margins/enzyme");
        let sibling = prefix.path().join("bin/enzyme");
        fake_enzyme(&sibling, "99.0.0", 0o755);
        fake_enzyme(&libexec, required_version(), 0o755);
        assert_eq!(
            select_bundled(&[sibling.clone(), libexec.clone()]).unwrap(),
            libexec
        );
        // The real order puts libexec first; a stale libexec copy falls
        // through to a matching archive sibling.
        let stale = prefix.path().join("stale/libexec/margins/enzyme");
        let archive = prefix.path().join("stale/bin/enzyme");
        fake_enzyme(&stale, "0.0.1", 0o755);
        fake_enzyme(&archive, required_version(), 0o755);
        assert_eq!(select_bundled(&[stale, archive.clone()]).unwrap(), archive);
    }

    #[test]
    fn an_installed_margins_never_considers_the_enzyme_beside_it() {
        let prefix = tempfile::tempdir().unwrap();
        let bin = prefix.path().join("bin");
        let home = prefix.path().join("home");
        // Archive layout: no ../libexec/margins, so the sibling is the engine.
        std::fs::create_dir_all(&bin).unwrap();
        assert_eq!(
            candidates_for(Some(&bin), &home),
            [bin.join("enzyme"), home.join("bin/enzyme")]
        );
        // Installed layout: only the libexec copy, even when it is missing and
        // the user's own enzyme beside margins reports the pinned version.
        let libexec = prefix.path().join("libexec/margins");
        std::fs::create_dir_all(&libexec).unwrap();
        fake_enzyme(&bin.join("enzyme"), required_version(), 0o755);
        let candidates = candidates_for(Some(&bin), &home);
        assert_eq!(candidates, [libexec.join("enzyme"), home.join("bin/enzyme")]);
        let message = select_bundled(&candidates).unwrap_err().to_string();
        assert!(!message.contains(&bin.join("enzyme").display().to_string()), "{message}");
    }

    #[test]
    fn no_usable_candidate_lists_each_rejection() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("libexec/margins/enzyme");
        let unexecutable = dir.path().join("bin/enzyme");
        let stale = dir.path().join("home/bin/enzyme");
        fake_enzyme(&unexecutable, required_version(), 0o644);
        fake_enzyme(&stale, "0.0.1", 0o755);
        let error =
            select_bundled(&[missing.clone(), unexecutable.clone(), stale.clone()]).unwrap_err();
        let message = error.to_string();
        for expected in [
            format!("{} (missing)", missing.display()),
            format!("{} (not executable)", unexecutable.display()),
            format!("{} (version 0.0.1)", stale.display()),
        ] {
            assert!(message.contains(&expected), "{message}");
        }
    }

    #[test]
    fn explicit_binary_must_exist() {
        let _guard = crate::test_process_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var_os(ENZYME_BIN_ENV);
        let missing = tempfile::tempdir().unwrap().path().join("enzyme");
        std::env::set_var(ENZYME_BIN_ENV, &missing);
        let error = locate_binary(Path::new("/nonexistent")).unwrap_err();
        match previous {
            Some(value) => std::env::set_var(ENZYME_BIN_ENV, value),
            None => std::env::remove_var(ENZYME_BIN_ENV),
        }
        assert!(error.to_string().contains(&missing.display().to_string()));
    }
}
