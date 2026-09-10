use std::path::Path;
use std::process::{Child, Command, Stdio};

pub struct FixtureGenerator {
    child: Child,
    _runtime: tempfile::TempDir,
    count_file: std::path::PathBuf,
}

impl FixtureGenerator {
    pub fn start(margins_home: &Path) -> Self {
        let runtime = tempfile::tempdir().unwrap();
        let port_file = runtime.path().join("port");
        let count_file = runtime.path().join("count");
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture_openai_server.py");
        let child = Command::new("python3")
            .arg(script)
            .args(["--port-file"])
            .arg(&port_file)
            .args(["--count-file"])
            .arg(&count_file)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if port_file.metadata().is_ok_and(|metadata| metadata.len() > 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let port = std::fs::read_to_string(&port_file)
            .unwrap()
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
