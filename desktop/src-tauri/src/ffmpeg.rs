use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

fn probe(path: &Path) -> bool {
    Command::new(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn automatic_candidates() -> Vec<PathBuf> {
    let name = binary_name();
    let mut candidates = Vec::new();

    // A hosted deployment can be a self-contained directory containing the
    // server and its ffmpeg sidecar.
    if let Ok(executable) = std::env::current_exe() {
        if let Some(parent) = executable.parent() {
            candidates.push(parent.join(name));
        }
    }

    // Non-login systemd services commonly omit ~/.local/bin from PATH even
    // when that is where the host provisioned its static ffmpeg binary.
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local").join("bin").join(name));
    }

    candidates.extend(
        [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    candidates.push(PathBuf::from(name));
    candidates
}

fn resolve_from(
    explicit: Option<PathBuf>,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        if probe(&path) {
            return Ok(path);
        }
        return Err(format!(
            "FFMPEG_BIN points to an unusable ffmpeg executable: {}",
            path.display()
        ));
    }

    for candidate in candidates {
        if probe(&candidate) {
            return Ok(candidate);
        }
    }

    Err(
        "ffmpeg is unavailable on the Margins host. Normal browser capture finalization does not need ffmpeg; deploy it beside margins-server, set FFMPEG_BIN, or install it only for imports or MARGINS_HOSTED_WEBM_FINALIZER=ffmpeg compatibility mode."
            .to_string(),
    )
}

/// Resolve and execute-probe the ffmpeg used by imports and the explicit hosted
/// WebM compatibility fallback. An explicit override is strict; automatic
/// discovery supports a sidecar beside the server, ~/.local/bin, common system
/// locations, and finally PATH.
pub(crate) fn resolve_binary() -> Result<PathBuf, String> {
    let explicit = std::env::var_os("FFMPEG_BIN")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    resolve_from(explicit, automatic_candidates())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "margins-ffmpeg-test-{}-{}",
                std::process::id(),
                NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fake_ffmpeg(dir: &Path, name: &str, exit_code: i32) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[test]
    fn explicit_override_is_strict() {
        let dir = TestDir::new();
        let broken = fake_ffmpeg(dir.path(), "broken-ffmpeg", 1);
        let fallback = fake_ffmpeg(dir.path(), "working-ffmpeg", 0);

        let error = resolve_from(Some(broken.clone()), [fallback]).unwrap_err();
        assert!(error.contains("FFMPEG_BIN"));
        assert!(error.contains(&broken.display().to_string()));
    }

    #[test]
    fn automatic_resolution_skips_broken_candidates() {
        let dir = TestDir::new();
        let broken = fake_ffmpeg(dir.path(), "broken-ffmpeg", 1);
        let working = fake_ffmpeg(dir.path(), "working-ffmpeg", 0);

        assert_eq!(
            resolve_from(None, [broken, working.clone()]).unwrap(),
            working
        );
    }

    #[test]
    fn missing_binary_explains_the_host_boundary() {
        let error = resolve_from(None, Vec::<PathBuf>::new()).unwrap_err();
        assert!(error.contains("Margins host"));
        assert!(error.contains("Normal browser capture finalization does not need ffmpeg"));
        assert!(error.contains("compatibility mode"));
    }
}
