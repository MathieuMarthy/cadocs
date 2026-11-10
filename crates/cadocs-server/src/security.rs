//! Headers for every response, as listed in `Security headers` in `docs/OPERATIONS.md`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::{
    CONTENT_SECURITY_POLICY, HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::response::Response;

/// The values every response gets, fixed at startup.
pub struct SecurityHeaders([(HeaderName, HeaderValue); 3]);

impl SecurityHeaders {
    /// `files_origin` is where the file store sends browsers for attachments, if anywhere: images
    /// then load from there (ADR 0020).
    pub fn new(files_origin: Option<&str>) -> Self {
        let images = match files_origin {
            Some(origin) => format!("'self' data: {origin}"),
            None => "'self' data:".to_owned(),
        };
        let policy = format!(
            "default-src 'self'; img-src {images}; style-src 'self' 'unsafe-inline'; \
             connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
        );
        Self([
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::try_from(policy).expect("an origin from a parsed URL"),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (REFERRER_POLICY, HeaderValue::from_static("same-origin")),
        ])
    }
}

/// A handler that sets one of these keeps its own value: public pages need a stricter policy.
pub async fn add_headers(
    State(security): State<Arc<SecurityHeaders>>,
    mut response: Response,
) -> Response {
    let headers = response.headers_mut();
    for (name, value) in &security.0 {
        headers.entry(name).or_insert_with(|| value.clone());
    }
    response
}
