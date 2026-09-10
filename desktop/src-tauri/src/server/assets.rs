// ---------------------------------------------------------------------------
// assets.rs — embedded static frontend assets (WP2)
//
// Embeds everything in `desktop/dist` at compile time via rust-embed.
// Serves a SPA with index.html fallback.  When serving index.html, injects
// `window.__MARGINS_TOKEN__` so the frontend can authenticate WS + API calls.
// ---------------------------------------------------------------------------

use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

#[derive(rust_embed::Embed)]
#[folder = "../dist"]
pub struct Assets;

/// Derive a simple MIME type from the file extension.
fn mime_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "application/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("webmanifest") => "application/manifest+json",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        _ => "application/octet-stream",
    }
}

/// Serve an embedded static asset for the given URI path.
/// Falls back to `index.html` for unknown paths (SPA routing).
/// Injects `window.__MARGINS_TOKEN__` into index.html responses.
pub fn serve_asset(uri_path: &str, token: &str) -> Response {
    let path = uri_path.trim_start_matches('/');

    // Try exact path; if not found, fall back to index.html.
    let asset_path = if Assets::get(path).is_some() {
        path.to_string()
    } else {
        "index.html".to_string()
    };

    match Assets::get(&asset_path) {
        Some(content) => {
            let mime = mime_for_path(&asset_path);
            let body = content.data;

            if asset_path == "index.html" {
                if !super::auth::is_valid_token(token) {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "invalid hosted authentication token",
                    )
                        .into_response();
                }
                let html = String::from_utf8_lossy(&body);
                let serialized = serde_json::to_string(token)
                    .expect("a validated hosted token is always JSON serializable");
                let injection = format!("<script>window.__MARGINS_TOKEN__={serialized};</script>");
                let patched = html.replacen("</head>", &format!("{injection}</head>"), 1);
                (
                    [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                    patched,
                )
                    .into_response()
            } else {
                ([(header::CONTENT_TYPE, mime)], body.into_owned()).into_response()
            }
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn index_injects_only_a_json_serialized_hex_token() {
        let token = "A1".repeat(32);
        let response = serve_asset("/", &token);
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(&format!(
            "<script>window.__MARGINS_TOKEN__={};</script>",
            serde_json::to_string(&token).unwrap()
        )));
    }

    #[tokio::test]
    async fn index_rejects_an_adversarial_non_hex_token() {
        let response = serve_asset("/", "</script><script>globalThis.pwned=true</script>");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(!text.contains("<script>"));
        assert!(!text.contains("globalThis.pwned"));
    }
}
