use serde::Serialize;
use std::io::{Read, Write};
use std::process::Command;
use std::time::Duration;

use crate::settings::pi_auth_path;

#[derive(Serialize)]
pub(crate) struct AiStatus {
    pub(crate) chatgpt_authenticated: bool,
    pub(crate) chatgpt_message: String,
}

struct MarginsOAuthCallbackServer {
    rx: std::sync::mpsc::Receiver<String>,
    _handle: std::thread::JoinHandle<()>,
}

fn start_margins_oauth_callback_server(
    redirect_uri: &str,
) -> Result<MarginsOAuthCallbackServer, String> {
    let host = redirect_uri
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .ok_or_else(|| format!("Cannot parse ChatGPT login callback URL: {redirect_uri}"))?;
    let port = host
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
        .ok_or_else(|| format!("Cannot parse ChatGPT login callback port: {redirect_uri}"))?;
    let listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("port {port}: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let handle = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 4096];
        let request_path = stream
            .read(&mut buf)
            .ok()
            .and_then(|n| {
                String::from_utf8_lossy(&buf[..n])
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1).map(str::to_string))
            })
            .unwrap_or_default();
        let html = r#"<!DOCTYPE html><html><head><title>Margins — ChatGPT connected</title></head>
<body style="font-family:system-ui,sans-serif;text-align:center;padding:60px 20px;background:#f8f9fa">
<h1 style="color:#2d7d46">✓ ChatGPT connected</h1>
<p>You can close this tab and return to Margins.</p>
</body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
            html.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
        let _ = tx.send(request_path);
    });
    Ok(MarginsOAuthCallbackServer {
        rx,
        _handle: handle,
    })
}

/// Best-effort proactive refresh of any expired pi OAuth tokens (incl.
/// "openai-codex"). This is a no-op with no network call unless a token is
/// within pi's proactive-refresh window. All errors (offline, etc.) are
/// swallowed so it can never break status reads or the distill handoff.
pub(crate) async fn refresh_pi_tokens() {
    if let Ok(mut auth) = pi::auth::AuthStorage::load(pi_auth_path()) {
        if auth.refresh_expired_oauth_tokens().await.is_ok() {
            let _ = auth.save();
        }
    }
}

pub(crate) async fn get_ai_status() -> Result<AiStatus, String> {
    refresh_pi_tokens().await;
    let auth = pi::auth::AuthStorage::load(pi_auth_path())
        .map_err(|e| format!("failed to read model login status: {e}"))?;
    let status = auth.credential_status("openai-codex");
    let chatgpt_authenticated = matches!(
        status,
        pi::auth::CredentialStatus::OAuthValid { .. } | pi::auth::CredentialStatus::ApiKey
    );
    Ok(AiStatus {
        chatgpt_authenticated,
        chatgpt_message: if chatgpt_authenticated {
            "ChatGPT login is connected.".to_string()
        } else {
            "Sign in with ChatGPT Plus/Pro, or use an API key below.".to_string()
        },
    })
}

pub(crate) async fn sign_in_chatgpt() -> Result<AiStatus, String> {
    let start = pi::auth::start_openai_codex_oauth()
        .map_err(|e| format!("failed to start ChatGPT login: {e}"))?;
    let redirect_uri = start
        .redirect_uri
        .clone()
        .ok_or_else(|| "ChatGPT login did not provide a redirect URI".to_string())?;
    let server = start_margins_oauth_callback_server(&redirect_uri)
        .map_err(|e| format!("failed to listen for ChatGPT login callback: {e}"))?;

    Command::new("open")
        .arg(&start.url)
        .status()
        .map_err(|e| format!("failed to open ChatGPT login in browser: {e}"))?;

    let callback_path = crate::async_runtime::spawn_blocking(move || {
        let keep_server_alive = server;
        keep_server_alive
            .rx
            .recv_timeout(Duration::from_secs(300))
            .map_err(|_| "Timed out waiting for ChatGPT login callback".to_string())
    })
    .await
    .map_err(|e| format!("ChatGPT login task failed: {e}"))??;

    let callback_url = if callback_path.starts_with("http") {
        callback_path
    } else {
        format!("http://localhost{callback_path}")
    };
    let credential = pi::auth::complete_openai_codex_oauth(&callback_url, &start.verifier)
        .await
        .map_err(|e| format!("failed to complete ChatGPT login: {e}"))?;
    let mut auth = pi::auth::AuthStorage::load(pi_auth_path())
        .map_err(|e| format!("failed to open login store: {e}"))?;
    let _ = auth.remove_provider_aliases("openai-codex");
    auth.set("openai-codex".to_string(), credential);
    auth.save()
        .map_err(|e| format!("failed to save ChatGPT login: {e}"))?;

    get_ai_status().await
}
