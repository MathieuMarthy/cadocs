//! The front end, embedded in the binary (ADR 0004).

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::{Embed, RustEmbed};

/// The Vite build. In debug builds `rust-embed` reads this folder at run time instead.
#[derive(Embed)]
#[folder = "../../web/dist"]
pub struct WebAssets;

pub async fn serve<E: RustEmbed>(uri: Uri) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let (path, file) = match E::get(path) {
        Some(file) => (path, file),
        None if is_front_end_route(path) => match E::get("index.html") {
            Some(file) => ("index.html", file),
            None => return StatusCode::NOT_FOUND.into_response(),
        },
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let mut response = Response::new(Body::from(file.data));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    // Vite puts a content hash in the name of everything under `assets/`.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// The front end routes in the browser, so a link to one of its pages must load it. A path with
/// an extension, or under a folder of files or of the server, is a file that is missing. The
/// front end's router leaves the same folders to the browser (`web/src/lib/router.svelte.ts`).
fn is_front_end_route(path: &str) -> bool {
    let first = path.split('/').next().unwrap_or_default();
    let last = path.rsplit('/').next().unwrap_or_default();
    !matches!(first, "api" | "assets" | "licenses" | "ws") && !last.contains('.')
}

/// The few types the build produces, which does not justify a MIME table in the binary.
fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}
