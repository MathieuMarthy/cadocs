//! Headers for every response, as listed in `Security headers` in `docs/OPERATIONS.md`.

use axum::http::HeaderValue;
use axum::http::header::{
    CONTENT_SECURITY_POLICY, HeaderName, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
};
use axum::response::Response;

const HEADERS: [(HeaderName, &str); 3] = [
    (
        CONTENT_SECURITY_POLICY,
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
         connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
    ),
    (X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (REFERRER_POLICY, "same-origin"),
];

/// A handler that sets one of these keeps its own value: public pages need a stricter policy.
pub async fn add_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    for (name, value) in HEADERS {
        headers
            .entry(name)
            .or_insert(HeaderValue::from_static(value));
    }
    response
}
