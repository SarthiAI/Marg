//! Static asset serving for the Marg Console. Files are embedded at compile
//! time from `marg/console/dist/` via `build.rs`. Operators run
//! `npm run build` inside `marg/console/` and the next `cargo build` picks
//! up the result.

use axum::extract::Path;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};

include!(concat!(env!("OUT_DIR"), "/console_embed.rs"));

const NOT_BUILT_HTML: &[u8] = b"<!doctype html><html><head><meta charset=\"utf-8\"><title>Marg Console</title></head><body style=\"font-family:system-ui;padding:32px;color:#444;\"><h1>Marg Console</h1><p>The console bundle was not embedded in this binary.</p><p>To build it: <code>cd marg/console &amp;&amp; npm install &amp;&amp; npm run build</code>, then rebuild marg with <code>cargo build --release</code>.</p><p>The API is fully available at <code>/admin/openapi.json</code>.</p></body></html>";

fn lookup(path: &str) -> Option<(&'static [u8], &'static str)> {
    for (p, bytes, mime) in CONSOLE_FILES {
        if *p == path {
            return Some((*bytes, *mime));
        }
    }
    None
}

fn cache_headers(path: &str) -> HeaderValue {
    // index.html: never cache (so a new bundle hash is picked up immediately).
    // hashed assets under assets/: long-lived immutable cache.
    if path == "index.html" {
        HeaderValue::from_static("no-store")
    } else if path.starts_with("assets/") {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    } else {
        HeaderValue::from_static("public, max-age=3600")
    }
}

fn render(path: &str) -> Response {
    if CONSOLE_FILE_COUNT == 0 {
        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return (StatusCode::OK, h, NOT_BUILT_HTML).into_response();
    }
    if let Some((bytes, mime)) = lookup(path) {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(mime) {
            h.insert(header::CONTENT_TYPE, v);
        }
        h.insert(header::CACHE_CONTROL, cache_headers(path));
        return (StatusCode::OK, h, bytes).into_response();
    }
    // Fallback to index.html for unknown sub-paths (defensive; we use hash
    // routing so this should not be hit in normal flows).
    if let Some((bytes, mime)) = lookup("index.html") {
        let mut h = HeaderMap::new();
        if let Ok(v) = HeaderValue::from_str(mime) {
            h.insert(header::CONTENT_TYPE, v);
        }
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return (StatusCode::OK, h, bytes).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

pub async fn index() -> Response {
    render("index.html")
}

pub async fn asset(Path(rest): Path<String>) -> Response {
    let p = rest.trim_start_matches('/');
    if p.is_empty() {
        return render("index.html");
    }
    render(p)
}

pub async fn root_redirect() -> Redirect {
    Redirect::permanent("/console/")
}

pub async fn console_redirect() -> Redirect {
    Redirect::permanent("/console/")
}
