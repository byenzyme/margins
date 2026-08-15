use chrono::{DateTime, Local};
use margins_core::{AsrBackend, CaptureProvider, DiarizationBackend, EventSink};
use margins_workflows::project::ResolvedProject;
use std::ffi::OsStr;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::Arc;

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Local>;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Local> {
        Local::now()
    }
}

pub trait ProcessRunner: Send + Sync {
    fn run(&self, program: &OsStr, arguments: &[String], cwd: &Path) -> anyhow::Result<ExitStatus>;
}

#[derive(Debug, Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, program: &OsStr, arguments: &[String], cwd: &Path) -> anyhow::Result<ExitStatus> {
        Ok(std::process::Command::new(program)
            .args(arguments)
            .current_dir(cwd)
            .status()?)
    }
}

/// Project persistence is injectable so `run` does not discover process-global
/// settings on its own. The standard composition delegates to the public
/// project workflow.
pub trait ProjectService: Send + Sync {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>>;
    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject>;
    /// Resolve the vault for a session command git-style: an explicit selector
    /// wins, otherwise walk up from `cwd` for a `.margins/` folder, otherwise the
    /// current folder is the vault. The default implementation preserves
    /// selector/registry behavior so test doubles need not implement discovery.
    fn resolve_vault(
        &self,
        selector: Option<&str>,
        _cwd: &Path,
    ) -> anyhow::Result<ResolvedProject> {
        self.resolve(selector)
    }
    fn set_active(&self, selector: &str) -> anyhow::Result<ResolvedProject>;
    fn add(
        &self,
        path: &str,
        name: Option<&str>,
        inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject>;
}

/// Path-scoped compatibility store used by the public CLI while the richer
/// aggregate repository remains available to embedders through `margins-core`.
/// Keeping this port injected prevents parser/dispatch tests from depending on
/// process globals or a particular database owner.
pub trait SessionStore: Send + Sync {
    fn current(&self, margins_dir: &Path) -> anyhow::Result<Option<String>>;
    fn set_current(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<()>;
    fn exists(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<bool>;
    fn get(
        &self,
        margins_dir: &Path,
        session_id: &str,
    ) -> anyhow::Result<margins_store::legacy::SessionMeta>;
    fn list(&self, margins_dir: &Path) -> anyhow::Result<Vec<margins_store::legacy::SessionInfo>>;
    fn set_title(
        &self,
        margins_dir: &Path,
        session_id: &str,
        title: Option<String>,
    ) -> anyhow::Result<Option<String>>;
    fn start_time(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<DateTime<Local>>;
    fn next_segment_ordinal(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<i64>;
    fn create(
        &self,
        margins_dir: &Path,
        session_id: &str,
        started_at: &DateTime<Local>,
        memo_uri: &str,
    ) -> anyhow::Result<()>;
    fn append_segment(
        &self,
        margins_dir: &Path,
        session_id: &str,
        ordinal: i64,
        audio_uri: &str,
        offset_ms: i64,
    ) -> anyhow::Result<()>;
    fn set_segment_duration(
        &self,
        margins_dir: &Path,
        session_id: &str,
        ordinal: i64,
        duration_secs: f64,
    ) -> anyhow::Result<()>;
    fn register_artifact(
        &self,
        margins_dir: &Path,
        session_id: &str,
        kind: &str,
        ordinal: i64,
        uri: &str,
    ) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct LocalSessionStore;

impl SessionStore for LocalSessionStore {
    fn current(&self, margins_dir: &Path) -> anyhow::Result<Option<String>> {
        match std::fs::read_to_string(margins_dir.join("current")) {
            Ok(value) => Ok(Some(value.trim().to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn set_current(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<()> {
        std::fs::write(margins_dir.join("current"), format!("{session_id}\n"))?;
        Ok(())
    }

    fn exists(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<bool> {
        margins_store::legacy::session_exists(margins_dir, session_id)
    }

    fn get(
        &self,
        margins_dir: &Path,
        session_id: &str,
    ) -> anyhow::Result<margins_store::legacy::SessionMeta> {
        margins_store::legacy::get_session_meta(margins_dir, session_id)
    }

    fn list(&self, margins_dir: &Path) -> anyhow::Result<Vec<margins_store::legacy::SessionInfo>> {
        margins_store::legacy::list_sessions(margins_dir)
    }

    fn set_title(
        &self,
        margins_dir: &Path,
        session_id: &str,
        title: Option<String>,
    ) -> anyhow::Result<Option<String>> {
        margins_store::legacy::set_title(margins_dir, session_id, title)
    }

    fn start_time(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<DateTime<Local>> {
        margins_store::legacy::get_session_start_time(margins_dir, session_id)
    }

    fn next_segment_ordinal(&self, margins_dir: &Path, session_id: &str) -> anyhow::Result<i64> {
        margins_store::legacy::next_segment_index(margins_dir, session_id)
    }

    fn create(
        &self,
        margins_dir: &Path,
        session_id: &str,
        started_at: &DateTime<Local>,
        memo_uri: &str,
    ) -> anyhow::Result<()> {
        margins_store::legacy::create_session(margins_dir, session_id, started_at, memo_uri)
    }

    fn append_segment(
        &self,
        margins_dir: &Path,
        session_id: &str,
        ordinal: i64,
        audio_uri: &str,
        offset_ms: i64,
    ) -> anyhow::Result<()> {
        margins_store::legacy::add_segment(
            margins_dir,
            session_id,
            ordinal,
            audio_uri,
            offset_ms,
            None,
        )
    }

    fn set_segment_duration(
        &self,
        margins_dir: &Path,
        session_id: &str,
        ordinal: i64,
        duration_secs: f64,
    ) -> anyhow::Result<()> {
        margins_store::legacy::update_segment_duration(
            margins_dir,
            session_id,
            ordinal,
            duration_secs,
        )
    }

    fn register_artifact(
        &self,
        margins_dir: &Path,
        session_id: &str,
        kind: &str,
        ordinal: i64,
        uri: &str,
    ) -> anyhow::Result<()> {
        margins_store::legacy::upsert_session_artifact(
            margins_dir,
            session_id,
            kind,
            ordinal,
            uri,
            "durable",
            None,
        )
    }
}

#[derive(Debug, Default)]
pub struct SystemProjectService;

impl ProjectService for SystemProjectService {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        margins_workflows::project::list_projects()
    }

    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        margins_workflows::project::resolve_project(selector)
    }

    fn resolve_vault(
        &self,
        selector: Option<&str>,
        cwd: &Path,
    ) -> anyhow::Result<ResolvedProject> {
        margins_workflows::project::resolve_vault(selector, cwd)
    }

    fn set_active(&self, selector: &str) -> anyhow::Result<ResolvedProject> {
        margins_workflows::project::set_active_project(selector)
    }

    fn add(
        &self,
        path: &str,
        name: Option<&str>,
        inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        margins_workflows::project::add_project(path, name, inbox_folder)
    }
}

pub struct CliServices {
    pub capture: Arc<dyn CaptureProvider>,
    pub sessions: Arc<dyn SessionStore>,
    pub asr: Arc<dyn AsrBackend>,
    pub diarization: Arc<dyn DiarizationBackend>,
    pub events: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub processes: Arc<dyn ProcessRunner>,
    pub projects: Arc<dyn ProjectService>,
}

impl Default for CliServices {
    fn default() -> Self {
        Self {
            capture: Arc::new(margins_core::UnavailableCaptureProvider::default()),
            sessions: Arc::new(LocalSessionStore),
            asr: Arc::new(margins_media::providers::UnavailableAsr),
            diarization: Arc::new(margins_media::providers::UnavailableDiarization),
            events: Arc::new(margins_core::NoopEventSink),
            clock: Arc::new(SystemClock),
            processes: Arc::new(SystemProcessRunner),
            projects: Arc::new(SystemProjectService),
        }
    }
}

/// Compose feature-selected public model adapters without making parser and
/// dispatch ownership depend on a private application crate. Model assets use
/// explicit environment overrides plus the standard macOS CoreML location;
/// missing or invalid assets retain the stable typed-unavailable behavior.
pub fn standalone_services() -> CliServices {
    #[allow(unused_mut)]
    let mut services = CliServices::default();

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    if let Some(backend) = coreml_model_root().and_then(|path| {
        margins_media::providers::coreml::CoreMlAsrBackend::from_dir_auto(path).ok()
    }) {
        services.asr = Arc::new(backend);
    }

    #[cfg(feature = "parakeet-onnx")]
    if !services.asr.is_available() {
        if let Some(backend) = model_path_from_env("MARGINS_PARAKEET_MODEL_DIR").and_then(|path| {
            margins_media::providers::parakeet::ParakeetOnnxBackend::from_dir(
                path,
                margins_media::providers::parakeet::AsrModelKind::Tdt,
            )
            .ok()
        }) {
            services.asr = Arc::new(backend);
        }
    }

    #[cfg(feature = "polyvoice-diarization")]
    {
        services.diarization =
            Arc::new(margins_media::providers::PublicDiarizationBackend { max_speakers: None });
    }

    services
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn coreml_model_root() -> Option<std::path::PathBuf> {
    model_path_from_env("MARGINS_FLUID_COREML_MODEL_DIR").or_else(|| {
        std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(std::path::PathBuf::from)
            .map(|home| home.join("Library/Application Support/FluidAudio/Models"))
    })
}

#[cfg(any(
    feature = "parakeet-onnx",
    all(feature = "coreml-asr", target_os = "macos")
))]
fn model_path_from_env(name: &str) -> Option<std::path::PathBuf> {
    model_path(
        std::env::var_os(name)?,
        std::env::var_os("HOME").as_deref().map(Path::new),
    )
}

#[cfg(any(
    feature = "parakeet-onnx",
    all(feature = "coreml-asr", target_os = "macos"),
    test
))]
fn model_path(value: std::ffi::OsString, home: Option<&Path>) -> Option<std::path::PathBuf> {
    if value.to_string_lossy().trim().is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(value);
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::model_path;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    #[test]
    fn model_paths_ignore_empty_values_and_expand_home() {
        assert_eq!(
            model_path(OsString::from("  "), Some(Path::new("/home/test"))),
            None
        );
        assert_eq!(
            model_path(
                OsString::from("~/models/parakeet"),
                Some(Path::new("/home/test"))
            ),
            Some(PathBuf::from("/home/test/models/parakeet"))
        );
        assert_eq!(
            model_path(OsString::from("relative/models"), None),
            Some(PathBuf::from("relative/models"))
        );
    }
}
