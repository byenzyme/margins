//! A bb launch naming a Workspace this machine lacks is refused with a reason
//! the plugin can show, and nothing is created.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn get(port: u16, path: &str, token: &str) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    let status = response.split_whitespace().nth(1)?.parse().ok()?;
    let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    Some((status, body))
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(root: &Path, port: u16, bb: bool) -> Server {
    let mut command = Command::new(env!("CARGO_BIN_EXE_margins-server"));
    command
        .current_dir(root.join("launcher"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root.join("user"))
        .env("MARGINS_HOST", "127.0.0.1")
        .env("MARGINS_PORT", port.to_string())
        .env("MARGINS_DATA_DIR", root.join("data"))
        .env("MARGINS_WORK_DIR", root.join("launcher"))
        .env("MARGINS_HOME", root.join("margins-home"))
        .env("MARGINS_WORKSPACE", "missing")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if bb {
        command.env("MARGINS_BB_CAPTURE_WORKSPACE", "1");
    }
    Server(command.spawn().unwrap())
}

fn assert_nothing_created(root: &Path) {
    assert!(!root.join("margins-home/workspaces/missing").exists());
    assert!(!root.join("margins-home/configs/missing.enzyme").exists());
    assert!(std::fs::read_dir(root.join("launcher"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn bb_launch_for_missing_workspace_reports_why_and_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("launcher")).unwrap();
    std::fs::create_dir_all(root.join("user")).unwrap();
    let port = free_port();
    let _server = launch(root, port, true);

    let deadline = Instant::now() + Duration::from_secs(20);
    let token = loop {
        let token = std::fs::read_to_string(root.join("data/token")).unwrap_or_default();
        if !token.trim().is_empty()
            && get(port, "/health", "").is_some_and(|(status, _)| status == 200)
        {
            break token.trim().to_string();
        }
        assert!(Instant::now() < deadline, "server never became healthy");
        std::thread::sleep(Duration::from_millis(80));
    };

    let (status, body) = get(port, "/v1/capabilities", &token).unwrap();
    assert_eq!(status, 409, "{body}");
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "workspace_not_found");
    let message = envelope["error"]["message"].as_str().unwrap();
    assert!(message.contains("'missing'"), "{message}");
    assert!(message.contains("margins init"), "{message}");
    assert!(!root.join("data/service.json").exists());
    assert_nothing_created(root);
}

#[test]
fn operator_launch_for_missing_workspace_exits_and_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("launcher")).unwrap();
    std::fs::create_dir_all(root.join("user")).unwrap();
    let server = launch(root, free_port(), false);
    let output = server_output(server);
    assert!(!output.0.success());
    assert!(output.1.contains("workspace_not_found"), "{}", output.1);
    assert_nothing_created(root);
}

fn server_output(mut server: Server) -> (std::process::ExitStatus, String) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            let mut stderr = String::new();
            server
                .0
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            return (status, stderr);
        }
        assert!(Instant::now() < deadline, "server did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}
