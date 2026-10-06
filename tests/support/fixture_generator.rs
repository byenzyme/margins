use std::path::Path;
use std::process::{Child, Command, Stdio};

pub struct FixtureGenerator {
    child: Child,
    _runtime: tempfile::TempDir,
    count_file: std::path::PathBuf,
}

impl FixtureGenerator {
    pub fn start(margins_home: &Path) -> Self {
        Self::start_with_delay(margins_home, 0)
    }

    /// A generator that answers each completion after `delay_ms`.
    #[allow(dead_code)]
    pub fn start_with_delay(margins_home: &Path, delay_ms: u64) -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let port_file = runtime.path().join("port");
        let count_file = runtime.path().join("count");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture_openai_server.py");
        let mut child = Command::new("python3")
            .arg(script)
            .args(["--port-file"])
            .arg(&port_file)
            .args(["--count-file"])
            .arg(&count_file)
            .args(["--delay-ms", &delay_ms.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        // A cold Python startup on a busy macOS runner can take longer than a
        // second. Wait for the server's port, while reporting an early exit.
        for _ in 0..600 {
            if port_file.metadata().is_ok_and(|metadata| metadata.len() > 0) {
                break;
            }
            if let Some(status) = child.try_wait().unwrap() {
                panic!("fixture generator exited before publishing its port: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let port = std::fs::read_to_string(&port_file)
            .expect("fixture generator did not publish its port within 15 seconds")
            .trim()
            .parse::<u16>()
            .unwrap();
        margins::hosted_credentials::install_bundle(
            margins_home,
            "fixture-generator",
            "fixture-key",
            &format!("http://127.0.0.1:{port}/v1"),
            "fixture-catalyst-model",
            Some(4_102_444_800),
        )
        .unwrap();
        Self {
            child,
            _runtime: runtime,
            count_file,
        }
    }

    pub fn request_count(&self) -> usize {
        std::fs::read_to_string(&self.count_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }
}

impl Drop for FixtureGenerator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
